use super::*;
use std::os::unix::fs::PermissionsExt;

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{name}-{}", Uuid::new_v4()))
}

#[test]
fn completion_contracts_require_parse_checked_atomic_replacement() {
    let signal = Path::new("/runtime/processes/goal-agent.signal.json");

    for phase in [None, Some("plan"), Some("criticize"), Some("revise")] {
        let prompt = goal_agent_protocol_prompt("GOAL", signal, phase);
        assert!(prompt.contains("goal-agent.signal.json.tmp"));
        assert!(prompt.contains("parse-check"));
        assert!(prompt.contains("`jq .`"));
        assert!(prompt.contains("atomically replace"));
        assert!(prompt.contains("Never write the completion payload directly"));
    }
}

#[test]
fn changing_invalid_json_restarts_the_legacy_write_grace_period() {
    let root = unique_temp_dir("goal-agent-changing-partial-signal");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent-test.signal.json");
    let mut reader = SignalReader::new(Duration::from_millis(25));

    fs::write(&signal_path, r#"{"state":"#).unwrap();
    assert!(matches!(
        reader.take(&signal_path).unwrap(),
        SignalRead::Pending
    ));
    thread::sleep(Duration::from_millis(30));
    fs::write(&signal_path, r#"{"state":"completed"#).unwrap();
    assert!(matches!(
        reader.take(&signal_path).unwrap(),
        SignalRead::Pending
    ));
    thread::sleep(Duration::from_millis(30));
    let SignalRead::Rejected(diagnostic) = reader.take(&signal_path).unwrap() else {
        panic!("unchanged invalid JSON must become actionable");
    };
    assert!(diagnostic.contains("invalid JSON after 25 ms"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_signal_state_is_an_immediate_typed_schema_rejection() {
    let root = unique_temp_dir("goal-agent-unsupported-signal-state");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent-test.signal.json");
    fs::write(&signal_path, r#"{"state":"done"}"#).unwrap();

    let SignalRead::Rejected(diagnostic) = SignalReader::default().take(&signal_path).unwrap()
    else {
        panic!("unsupported state must be rejected by the typed signal schema");
    };
    assert!(diagnostic.contains("does not match the required schema"));
    assert!(diagnostic.contains("state"));
    assert!(signal_path.is_file());

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn live_goal_agent_recovers_from_stable_malformed_json_in_the_same_session() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let script = r#"#!/bin/sh
printf '%s' '{"state":"completed"' > "$REFINE_AGENT_SIGNAL_PATH"
IFS= read -r instruction
printf '%s\n' "$instruction" > recovery-instruction.txt
printf '%s\n' '{"state":"completed","message":"recovered completion"}' > "$REFINE_AGENT_SIGNAL_PATH.tmp"
mv "$REFINE_AGENT_SIGNAL_PATH.tmp" "$REFINE_AGENT_SIGNAL_PATH"
sleep 10
"#;
    let (root, runtime_root, result) = run_test_provider(
        "goal-agent-invalid-json-recovery",
        script,
        Duration::from_secs(6),
    );
    let result = result.unwrap();

    assert_eq!(result.output, "recovered completion");
    let archive = runtime_root
        .join("processes")
        .join(format!("{}.signal.invalid.1.json", result.process_id));
    assert_eq!(
        fs::read_to_string(&archive).unwrap(),
        r#"{"state":"completed""#
    );
    let diagnostic = archive.with_extension("error.txt");
    assert!(
        fs::read_to_string(&diagnostic)
            .unwrap()
            .contains("invalid JSON after 2000 ms")
    );
    let instruction = fs::read_to_string(root.join("app/recovery-instruction.txt")).unwrap();
    assert!(instruction.contains("replacement attempt 1 of 3"));
    assert!(instruction.contains("same required JSON shape"));
    assert!(instruction.contains("atomically rename"));

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn invalid_signal_recovery_fails_after_three_rejected_replacements() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let script = r#"#!/bin/sh
attempt=1
while [ "$attempt" -le 4 ]; do
  printf '%s\n' '{"state":7}' > "$REFINE_AGENT_SIGNAL_PATH"
  if [ "$attempt" -lt 4 ]; then
    IFS= read -r instruction
  fi
  attempt=$((attempt + 1))
done
sleep 10
"#;
    let (root, runtime_root, result) = run_test_provider(
        "goal-agent-invalid-schema-limit",
        script,
        Duration::from_secs(5),
    );
    let error = result.unwrap_err().to_string();

    assert!(error.contains("exhausted the limit of 3 rejected replacement payloads"));
    assert!(error.contains("does not match the required schema"));
    let process_dir = runtime_root.join("processes");
    let invalid_payloads = fs::read_dir(&process_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".signal.invalid.")
        })
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .count();
    let diagnostics = fs::read_dir(&process_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".error.txt"))
        .count();
    assert_eq!(invalid_payloads, 4);
    assert_eq!(diagnostics, 4);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn malformed_signal_is_preserved_with_a_terminal_diagnostic_when_agent_exits() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let script = r#"#!/bin/sh
printf '%s' '{"state":"completed"' > "$REFINE_AGENT_SIGNAL_PATH"
"#;
    let (root, runtime_root, result) = run_test_provider(
        "goal-agent-invalid-json-exit",
        script,
        Duration::from_secs(5),
    );
    let error = result.unwrap_err().to_string();

    assert!(error.contains("exited before it could rewrite the payload"));
    assert!(error.contains("invalid JSON"));
    let process_dir = runtime_root.join("processes");
    assert!(
        fs::read_dir(process_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".signal.invalid.1.json")
            })
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn agent_exit_while_a_replacement_is_outstanding_returns_the_archived_diagnostic() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let script = r#"#!/bin/sh
printf '%s\n' '{"state":7}' > "$REFINE_AGENT_SIGNAL_PATH"
IFS= read -r instruction
"#;
    let (root, _runtime_root, result) = run_test_provider(
        "goal-agent-exits-during-recovery",
        script,
        Duration::from_secs(5),
    );
    let error = result.unwrap_err().to_string();

    assert!(error.contains("exited before it produced a valid replacement"));
    assert!(error.contains("does not match the required schema"));
    assert!(error.contains(".signal.invalid.1.json"));
    assert!(error.contains(".signal.invalid.1.error.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
fn run_test_provider(
    name: &str,
    script: &str,
    completion_timeout: Duration,
) -> (PathBuf, PathBuf, RefineResult<GoalAgentResult>) {
    let root = unique_temp_dir(name);
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(&provider, script).unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let result = run_goal_agent(
        GoalAgentLaunch {
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test malformed completion recovery".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!(name))]),
            completion_timeout: Some(completion_timeout),
            idle_timeout: None,
        },
        |_| {},
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    (root, runtime_root, result)
}
