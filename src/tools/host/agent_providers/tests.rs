use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn host_provider_service_detects_known_provider_binaries() {
    let temp_root = unique_temp_dir("providers");
    let bin_dir = temp_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(bin_dir.join("codex"), "#!/bin/sh\n").unwrap();
    fs::write(bin_dir.join("smoke-ai"), "#!/bin/sh\n").unwrap();

    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        ..HostAgentProviderService::default()
    };
    let providers = service.detect().unwrap();
    let codex = providers
        .iter()
        .find(|provider| provider.name == "codex")
        .unwrap();
    assert!(codex.installed);
    assert!(codex.supports_resume);
    assert_eq!(codex.output_format, "codex_json");
    let smoke_ai = providers
        .iter()
        .find(|provider| provider.name == "smoke-ai")
        .unwrap();
    assert!(smoke_ai.installed);
    let claude = providers
        .iter()
        .find(|provider| provider.name == "claude")
        .unwrap();
    assert!(!claude.installed);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn interactive_provider_commands_keep_the_native_cli_conversation_mode() {
    let temp_root = unique_temp_dir("interactive-provider-command");
    let bin_dir = temp_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    for binary in ["claude", "codex", "gemini", "copilot", "smoke-ai"] {
        let path = bin_dir.join(binary);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        make_executable(&path);
    }
    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        ..HostAgentProviderService::default()
    };

    for (provider, expected_args) in [
        (
            "claude",
            vec!["--dangerously-skip-permissions", "initial context"],
        ),
        (
            "codex",
            vec![
                "--dangerously-bypass-approvals-and-sandbox",
                "initial context",
            ],
        ),
        ("gemini", vec!["--yolo", "-i", "initial context"]),
        ("copilot", vec!["--allow-all", "-i", "initial context"]),
        ("smoke-ai", vec!["initial context"]),
    ] {
        let command = service
            .interactive_command(provider, "initial context")
            .unwrap();
        assert_eq!(command.args, expected_args);
        assert!(
            !command
                .args
                .iter()
                .any(|arg| { matches!(arg.as_str(), "--print" | "exec" | "-p" | "--prompt") })
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn host_provider_service_invokes_smoke_ai_and_extracts_json_final_text() {
    let temp_root = unique_temp_dir("provider-invoke");
    let bin_dir = temp_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let smoke = bin_dir.join("smoke-ai");
    fs::write(
            &smoke,
            "#!/bin/sh\nprintf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"smoke ok\"}}'\n",
        )
        .unwrap();
    make_executable(&smoke);

    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        runtime_root: Some(temp_root.join("run/8080")),
    };
    let output = service
        .invoke(ProviderInvocation {
            provider: "smoke-ai".to_string(),
            prompt: "hello".to_string(),
            session_id: None,
            cwd: None,
            process_metadata: Default::default(),
        })
        .unwrap();
    assert!(output.contains("agent_message"));
    assert!(temp_root.join("run/8080/processes").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn host_provider_service_sends_large_codex_prompts_over_stdin() {
    let temp_root = unique_temp_dir("provider-large-codex-prompt");
    let bin_dir = temp_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex = bin_dir.join("codex");
    fs::write(
            &codex,
            concat!(
                "#!/bin/sh\n",
                "test \"$6\" = - || exit 2\n",
                "test \"$(wc -c)\" -eq 1048576 || exit 3\n",
                "printf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"large prompt received\"}}'\n",
            ),
        )
        .unwrap();
    make_executable(&codex);

    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        runtime_root: Some(temp_root.join("run/8080")),
    };
    for session_id in [None, Some("session-1".to_string())] {
        let result = service
            .invoke_detailed(ProviderInvocation {
                provider: "codex".to_string(),
                prompt: "x".repeat(1024 * 1024),
                session_id,
                cwd: None,
                process_metadata: Default::default(),
            })
            .unwrap();

        assert_eq!(result.output, "large prompt received");
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn extract_final_text_handles_codex_and_copilot_jsonl() {
    let codex = r#"{"item":{"type":"agent_message","text":"done"}}"#;
    assert_eq!(extract_final_text(codex, "codex_json"), "done");

    let copilot = concat!(
        "{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"hel\"}}\n",
        "{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"lo\"}}\n"
    );
    assert_eq!(extract_final_text(copilot, "copilot_json"), "hello");
}

#[test]
fn provider_activity_formatter_extracts_readable_stream_events() {
    let mut formatter = ProviderActivityFormatter::new("codex_json");
    let lines = formatter.push(
        ManagedProcessOutputStream::Stdout,
        b"{\"item\":{\"type\":\"agent_message\",\"text\":\"streamed agent text\"}}\n",
    );
    assert_eq!(lines, vec!["streamed agent text"]);

    let lines = formatter.push(
        ManagedProcessOutputStream::Stdout,
        b"{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"delta text\"}}\n",
    );
    assert_eq!(lines, vec!["delta text"]);
}

#[test]
fn provider_error_message_summarizes_codex_api_error() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":401,"result":"Invalid API key - Fix external API key"}"#;
    assert_eq!(
        provider_error_message(stdout, ""),
        Some("Invalid API key - Fix external API key (401)".to_string())
    );
}

#[test]
fn provider_error_message_ignores_success_with_null_api_status() {
    let stdout = r#"{"type":"result","subtype":"success","is_error":false,"api_error_status":null,"result":"Hello"}"#;
    assert_eq!(provider_error_message(stdout, ""), None);
}

#[test]
fn extract_provider_session_id_handles_common_jsonl_shapes() {
    let stdout = concat!(
        "{\"item\":{\"type\":\"agent_message\",\"text\":\"done\"},\"session_id\":\"prov-1\"}\n",
        "{\"data\":{\"conversationId\":\"prov-2\"}}\n"
    );
    assert_eq!(
        extract_provider_session_id(stdout),
        Some("prov-1".to_string())
    );
    assert_eq!(
        extract_provider_session_id("{\"data\":{\"conversationId\":\"prov-2\"}}\n"),
        Some("prov-2".to_string())
    );
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
