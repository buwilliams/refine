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
        runtime_root: Some(temp_root.join("run/8080")),
    };
    let providers = service.detect().unwrap();
    let codex = providers
        .iter()
        .find(|provider| provider.name == "codex")
        .unwrap();
    assert!(codex.installed);
    assert!(codex.supports_resume);
    assert_eq!(codex.output_format, "codex_json");
    assert_eq!(
        codex.prompt_transport,
        ProviderPromptCapability::NativeStdin
    );
    let smoke_ai = providers
        .iter()
        .find(|provider| provider.name == "smoke-ai")
        .unwrap();
    assert!(smoke_ai.installed);
    assert_eq!(
        smoke_ai.prompt_transport,
        ProviderPromptCapability::InlineOrFile
    );
    let claude = providers
        .iter()
        .find(|provider| provider.name == "claude")
        .unwrap();
    assert!(!claude.installed);
    for provider in providers.iter().filter(|provider| provider.name != "codex") {
        assert_eq!(
            provider.prompt_transport,
            ProviderPromptCapability::InlineOrFile,
            "{}",
            provider.name
        );
    }

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
        runtime_root: Some(temp_root.join("run/8080")),
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
fn supervised_launch_uses_final_environment_for_file_fallback_and_child_parity() {
    let _env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp_root = unique_temp_dir("provider-final-environment");
    let bin_dir = temp_root.join("bin");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&bin_dir).unwrap();
    let smoke = bin_dir.join("smoke-ai");
    fs::write(
        &smoke,
        concat!(
            "#!/bin/sh\n",
            "prompt_path=$(printf '%s' \"$1\" | sed -n '4{s/^`//;s/`$//;p;}')\n",
            "test -r \"$prompt_path\" || exit 2\n",
            "grep -q 'FINAL_ENV_PROMPT_SECRET' \"$prompt_path\" || exit 3\n",
            "test \"$REFINE_INHERITED_KEY\" = override || exit 4\n",
            "test \"$REFINE_DUPLICATE_KEY\" = final || exit 5\n",
            "test \"$REFINE_SESSION_ROLE\" = supervised || exit 6\n",
            "test \"$REFINE_MULTIBYTE\" = '🙂é' || exit 7\n",
            "test \"${OPENAI_API_KEY-unset}\" = unset || exit 8\n",
            "printf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"final environment prompt received\"}}'\n",
        ),
    )
    .unwrap();
    make_executable(&smoke);
    let previous_inherited = std::env::var_os("REFINE_INHERITED_KEY");
    let previous_api_key = std::env::var_os("OPENAI_API_KEY");
    unsafe {
        std::env::set_var("REFINE_INHERITED_KEY", "inherited");
        std::env::set_var("OPENAI_API_KEY", "must-be-removed");
    }
    let mut environment = (0..23)
        .map(|index| (format!("REFINE_LARGE_{index}"), "e".repeat(65_800)))
        .collect::<Vec<_>>();
    environment.extend([
        ("REFINE_INHERITED_KEY".to_string(), "override".to_string()),
        ("REFINE_DUPLICATE_KEY".to_string(), "first".to_string()),
        ("REFINE_DUPLICATE_KEY".to_string(), "final".to_string()),
        ("REFINE_SESSION_ROLE".to_string(), "supervised".to_string()),
        ("REFINE_MULTIBYTE".to_string(), "🙂é".to_string()),
        (
            "OPENAI_API_KEY".to_string(),
            "still-must-be-removed".to_string(),
        ),
    ]);
    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        runtime_root: Some(runtime_root.clone()),
    };
    let secret = "FINAL_ENV_PROMPT_SECRET";
    let prompt = format!("{secret}{}", "p".repeat(60_000 - secret.len()));
    let result = service
        .invoke_detailed_with_environment_and_output(
            ProviderInvocation {
                provider: "smoke-ai".to_string(),
                prompt,
                session_id: None,
                cwd: None,
                process_metadata: Default::default(),
            },
            &environment,
            |_| {},
        )
        .unwrap();

    assert!(result.output.contains("final environment prompt received"));
    assert!(
        fs::read_dir(runtime_root.join("agent-prompts"))
            .unwrap()
            .next()
            .is_none()
    );
    unsafe {
        match previous_inherited {
            Some(value) => std::env::set_var("REFINE_INHERITED_KEY", value),
            None => std::env::remove_var("REFINE_INHERITED_KEY"),
        }
        match previous_api_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn effective_environment_rejects_before_supervised_spawn_without_prompt_disclosure() {
    let temp_root = unique_temp_dir("provider-environment-rejection");
    let bin_dir = temp_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let marker = temp_root.join("spawned");
    let smoke = bin_dir.join("smoke-ai");
    fs::write(
        &smoke,
        format!("#!/bin/sh\nprintf spawned > '{}'\n", marker.display()),
    )
    .unwrap();
    make_executable(&smoke);
    let environment = (0..24)
        .map(|index| (format!("REFINE_TOO_LARGE_{index}"), "e".repeat(65_800)))
        .collect::<Vec<_>>();
    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        runtime_root: Some(temp_root.join("run/8080")),
    };
    let secret = "PRESPAWN_SECRET_MUST_NOT_LEAK";
    let prompt = format!("{secret}{}", "p".repeat(60_000 - secret.len()));
    let error = service
        .invoke_detailed_with_environment_and_output(
            ProviderInvocation {
                provider: "smoke-ai".to_string(),
                prompt,
                session_id: None,
                cwd: None,
                process_metadata: Default::default(),
            },
            &environment,
            |_| {},
        )
        .unwrap_err();
    let message = error.to_string();

    assert!(message.contains("effective-environment budget before spawn"));
    assert!(!message.contains(secret));
    assert!(!marker.exists());
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

#[cfg(unix)]
#[test]
fn oversized_argv_provider_uses_exact_prompt_file_without_metadata_disclosure() {
    let temp_root = unique_temp_dir("provider-large-file-prompt");
    let bin_dir = temp_root.join("bin");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&bin_dir).unwrap();
    let smoke = bin_dir.join("smoke-ai");
    fs::write(
        &smoke,
        concat!(
            "#!/bin/sh\n",
            "test \"$(printf '%s' \"$1\" | wc -c)\" -lt 4096 || exit 2\n",
            "prompt_path=$(printf '%s' \"$1\" | sed -n '4{s/^`//;s/`$//;p;}')\n",
            "test -r \"$prompt_path\" || exit 3\n",
            "test \"$(wc -c < \"$prompt_path\")\" -eq 158078 || exit 4\n",
            "test \"$(sha256sum \"$prompt_path\" | cut -d' ' -f1)\" = ",
            "\"$(printf '%s' \"$1\" | sed -n '8{s/^- SHA-256: `//;s/`$//;p;}')\" || exit 5\n",
            "printf '%s' \"$1\" > \"$0.argv\"\n",
            "printf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"file prompt received\"}}'\n",
        ),
    )
    .unwrap();
    make_executable(&smoke);

    let secret = "ROUND7_SECRET_";
    let prompt = format!("{secret}{}", "x".repeat(158_078 - secret.len()));
    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        runtime_root: Some(runtime_root.clone()),
    };
    let result = service
        .invoke_detailed(ProviderInvocation {
            provider: "smoke-ai".to_string(),
            prompt,
            session_id: None,
            cwd: None,
            process_metadata: Default::default(),
        })
        .unwrap();
    assert!(result.output.contains("file prompt received"));
    let captured_argv = fs::read_to_string(format!("{}.argv", smoke.display())).unwrap();
    assert!(!captured_argv.contains(secret));
    assert!(captured_argv.contains("complete authoritative task prompt"));
    assert!(
        fs::read_dir(runtime_root.join("agent-prompts"))
            .unwrap()
            .next()
            .is_none()
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn portable_pty_preserves_original_exec_errno() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new("/bin/true");
    command.arg("x".repeat(200_000));
    let error = pair.slave.spawn_command(command).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("Argument list too long") || message.contains("os error 7"),
        "original E2BIG was not retained: {message}"
    );
    assert!(!message.contains("output.write"), "{message}");
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
