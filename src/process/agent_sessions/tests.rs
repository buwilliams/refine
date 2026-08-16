use super::*;
use crate::process::subprocess::ProcessSupervisor;
use std::os::unix::fs::PermissionsExt;

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{name}-{}", Uuid::new_v4()))
}

#[test]
fn workflow_goal_agent_prompt_excludes_interactive_checkout_guidance() {
    let spec = "# Goal Agent Specification\n\n## Latest Round\n\n### Request\n\nLATEST_SENTINEL";
    let prompt = goal_agent_protocol_prompt(
        spec,
        Path::new("/runtime/processes/goal-agent.signal.json"),
        None,
    );

    assert!(!prompt.contains("Active Refine executable"));
    assert!(!prompt.contains("checkout-local `./r`"));
    assert!(prompt.contains("/runtime/processes/goal-agent.signal.json"));
    assert!(!prompt.contains("{{"));
    assert_eq!(prompt.matches("# Goal Agent Specification").count(), 1);
    assert_eq!(prompt.matches("LATEST_SENTINEL").count(), 1);
    assert!(
        prompt.find("guidance_applied").unwrap()
            < prompt.find("# Goal Agent Specification").unwrap()
    );
    assert!(prompt.trim_end().ends_with("LATEST_SENTINEL"));
}

#[test]
fn planning_sessions_require_the_phase_specific_structured_result() {
    let signal = Path::new("/runtime/processes/goal-agent.signal.json");
    let plan = goal_agent_protocol_prompt("PLAN_SENTINEL", signal, Some("plan"));
    let criticize = goal_agent_protocol_prompt("CRITICIZE_SENTINEL", signal, Some("criticize"));
    let revise = goal_agent_protocol_prompt("REVISE_SENTINEL", signal, Some("revise"));

    for prompt in [&plan, &criticize, &revise] {
        assert!(prompt.contains("required `planning_result`"));
        assert!(prompt.contains("never omitted or quoted as a string"));
        assert!(prompt.contains("zero-based integer indexes"));
        assert!(prompt.contains("never use names"));
        assert!(prompt.contains("\"guidance_applied\":[0]"));
        assert!(!prompt.contains("implementation_evidence"));
        assert!(prompt.contains(signal.to_str().unwrap()));
        assert!(!prompt.contains("{{"));
    }
    assert!(plan.contains("\"checklist\""));
    assert!(criticize.contains("\"findings\""));
    assert!(revise.contains("\"criticism_resolutions\""));
    assert!(revise.contains("\"criticism_id\":\"C1\""));
    assert!(revise.contains("\"resolution\":\"how the revised plan resolves"));
}

#[test]
fn completion_signal_rejects_guidance_names_as_a_schema_error() {
    let root = unique_temp_dir("goal-agent-invalid-guidance-signal");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent.signal.json");
    fs::write(
        &signal_path,
        r#"{"state":"completed","message":"planned","guidance_applied":["Intent and architecture"],"planning_result":{"summary":"Plan summary.","checklist":[]}}"#,
    )
    .unwrap();

    let read = SignalReader::default().take(&signal_path).unwrap();
    let SignalRead::Rejected(diagnostic) = read else {
        panic!("schema-invalid signal must be rejected");
    };

    assert!(diagnostic.contains("does not match the required schema"));
    assert!(diagnostic.contains("guidance_applied"));
    assert!(signal_path.is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_signal_allows_partial_json_only_for_the_write_grace_period() {
    let root = unique_temp_dir("goal-agent-partial-signal");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent.signal.json");
    fs::write(&signal_path, r#"{"state":"completed"#).unwrap();
    let mut reader = SignalReader::new(Duration::ZERO);

    assert!(matches!(
        reader.take(&signal_path).unwrap(),
        SignalRead::Pending
    ));
    let SignalRead::Rejected(diagnostic) = reader.take(&signal_path).unwrap() else {
        panic!("stable invalid JSON must be rejected after the grace period");
    };

    assert!(diagnostic.contains("invalid JSON after 0 ms"));
    assert!(signal_path.is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn planning_completion_signals_must_carry_the_planning_result_object() {
    let root = unique_temp_dir("goal-agent-omitted-planning-result");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent.signal.json");
    fs::write(
        &signal_path,
        r#"{"state":"completed","message":"revised the plan","guidance_applied":[]}"#,
    )
    .unwrap();

    let mut reader = SignalReader::new(Duration::ZERO).requiring_planning_result(true);
    let SignalRead::Rejected(diagnostic) = reader.take(&signal_path).unwrap() else {
        panic!("a planning completion without planning_result must be rejected");
    };
    assert!(diagnostic.contains("omitted the required planning_result object"));
    // The rejected payload stays on disk for recovery to archive.
    assert!(signal_path.is_file());

    let mut lenient = SignalReader::new(Duration::ZERO);
    assert!(matches!(
        lenient.take(&signal_path).unwrap(),
        SignalRead::Valid(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn planning_session_returns_the_structured_result_from_its_completion_signal() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-planning-result");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\nprintf '%s\\n' '{\"state\":\"completed\",\"message\":\"planned\",\"guidance_applied\":[],\"planning_result\":{\"summary\":\"Change the boundary so debug workers use the active executable.\",\"checklist\":[{\"id\":\"P1\",\"description\":\"Pass the active executable to supervised workers.\"}]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\nsleep 10\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let mut metadata = Map::new();
    metadata.insert("goal_id".to_string(), json!("GOAL-PLAN"));
    metadata.insert("implementation_phase".to_string(), json!("plan"));
    let result = run_goal_agent(
        GoalAgentLaunch {
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "plan".to_string(),
            metadata,
            completion_timeout: None,
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(result.output, "planned");
    assert_eq!(
        result.planning_result,
        Some(json!({
            "summary": "Change the boundary so debug workers use the active executable.",
            "checklist": [{
                "id": "P1",
                "description": "Pass the active executable to supervised workers."
            }]
        }))
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn goal_agent_hard_cap_terminates_a_session_without_a_completion_signal() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-completion-timeout");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(&provider, "#!/bin/sh\nsleep 10\n").unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let started_at = std::time::Instant::now();
    let error = run_goal_agent(
        GoalAgentLaunch {
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test timeout".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-TIMEOUT"))]),
            completion_timeout: Some(Duration::from_millis(200)),
        },
        |_| {},
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("did not produce a valid completion signal")
    );
    assert!(started_at.elapsed() < Duration::from_secs(2));
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .is_empty()
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_goal_agent_providers_receive_the_same_composed_specification() {
    let root = unique_temp_dir("goal-agent-provider-prompt");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    for binary in [
        "claude",
        "codex",
        "gemini",
        "copilot",
        "smoke-ai",
        "custom-agent",
    ] {
        let path = bin_dir.join(binary);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
    let service = HostAgentProviderService {
        path_override: Some(bin_dir.display().to_string()),
        runtime_root: Some(root.join("run/8082/agents")),
    };
    let spec =
        "# Goal Agent Specification\n\n## Latest Round\n\n### Request\n\nLATEST_PROVIDER_SENTINEL";
    let prompt = goal_agent_protocol_prompt(
        spec,
        Path::new("/runtime/processes/goal-agent.signal.json"),
        None,
    );

    for provider in [
        "claude",
        "codex",
        "gemini",
        "copilot",
        "smoke-ai",
        "custom-agent",
    ] {
        let command = service.interactive_command(provider, &prompt).unwrap();
        assert_eq!(
            command
                .args
                .iter()
                .filter(|argument| argument.as_str() == prompt)
                .count(),
            1,
            "{provider} must receive the one shared composed prompt"
        );
        assert!(command.stdin.is_none());
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_goal_agent_is_discoverable_and_attachable_while_running() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-session");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\nprintf 'ready\\n'\nread answer\nprintf 'answer:%s\\n' \"$answer\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let runtime_for_thread = runtime_root.clone();
    let app_for_thread = app_root.clone();
    let run = thread::spawn(move || {
        let mut metadata = Map::new();
        metadata.insert("goal_id".to_string(), json!("GOAL1"));
        run_goal_agent(
            GoalAgentLaunch {
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "test".to_string(),
                metadata,
                completion_timeout: None,
            },
            |_| {},
        )
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        if let Ok(snapshot) = find_goal_agent_session(&runtime_root, "GOAL1") {
            break snapshot;
        }
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    };
    assert!(snapshot.alive);
    let transcript = loop {
        let events = agent_session_events_since(&runtime_root, &snapshot.id, 0).unwrap();
        let transcript = events
            .iter()
            .filter_map(|event| event.get("data").and_then(Value::as_str))
            .collect::<String>();
        if transcript.contains("ready") {
            break transcript;
        }
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    };
    let transcript_len = transcript.len() as u64;
    let middle =
        agent_session_events_range(&runtime_root, &snapshot.id, 1, Some(transcript_len - 1))
            .unwrap();
    assert_eq!(
        middle[0]["data"].as_str().unwrap().as_bytes(),
        &transcript.as_bytes()[1..transcript.len() - 1],
    );
    assert!(
        find_agent_session(&runtime_root, &snapshot.id)
            .unwrap()
            .transcript_bytes
            >= transcript_len
    );
    let mut duplicate_metadata = Map::new();
    duplicate_metadata.insert("goal_id".to_string(), json!("GOAL1"));
    let duplicate = run_goal_agent(
        GoalAgentLaunch {
            runtime_root: runtime_root.clone(),
            cwd: app_root.clone(),
            provider: "smoke-ai".to_string(),
            prompt: "duplicate".to_string(),
            metadata: duplicate_metadata,
            completion_timeout: None,
        },
        |_| {},
    );
    assert!(matches!(duplicate, Err(RefineError::Conflict(_))));
    send_agent_session_input(&runtime_root, &snapshot.id, "hello\r").unwrap();
    let result = run.join().unwrap().unwrap();
    assert!(result.output.contains("answer:hello"));
    assert!(matches!(
        find_agent_session(&runtime_root, &snapshot.id),
        Err(RefineError::NotFound(_))
    ));

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_goal_agent_surfaces_needs_input_and_continues_same_session() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-needs-input");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
            &provider,
            "#!/bin/sh\n\
             printf '%s\\n' '{\"state\":\"needs_input\",\"message\":\"Choose the public name\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
             read answer\n\
             printf 'chosen:%s\\n' \"$answer\"\n\
             printf '%s\\n' '{\"state\":\"completed\",\"message\":\"Implemented and verified the selected public name.\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
             sleep 10\n",
        )
        .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let runtime_for_thread = runtime_root.clone();
    let app_for_thread = app_root.clone();
    let (attention_tx, attention_rx) = std::sync::mpsc::channel();
    let run = thread::spawn(move || {
        let mut metadata = Map::new();
        metadata.insert("goal_id".to_string(), json!("GOAL2"));
        run_goal_agent(
            GoalAgentLaunch {
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "test".to_string(),
                metadata,
                completion_timeout: None,
            },
            |attention| {
                let _ = attention_tx.send(attention);
            },
        )
    });

    let attention = attention_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(attention.message, "Choose the public name");
    let snapshot = find_goal_agent_session(&runtime_root, "GOAL2").unwrap();
    assert_eq!(snapshot.attention_state.as_deref(), Some("needs_input"));
    assert_eq!(
        snapshot.attention_message.as_deref(),
        Some("Choose the public name")
    );
    send_agent_session_input(&runtime_root, &snapshot.id, "Refine\r").unwrap();
    let result = run.join().unwrap().unwrap();
    assert_eq!(
        result.output,
        "Implemented and verified the selected public name."
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_goal_agent_handoff_survives_dead_process_recovery() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-recovery-handoff");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\n\
             printf 'transcript survives recovery\\n'\n\
             printf '%s\\n' '{\"state\":\"completed\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
             sleep 10\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let mut metadata = Map::new();
    metadata.insert("goal_id".to_string(), json!("GOAL-RECOVERY"));
    let result = run_goal_agent_session(
        GoalAgentLaunch {
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test".to_string(),
            metadata,
            completion_timeout: None,
        },
        |_| {},
        |supervisor, process, settlement| {
            assert!(!FileProcessSupervisor::process_is_alive(process)?);
            assert_eq!(settlement.process_id, process.id);
            assert!(settlement.output.contains("transcript survives recovery"));
            let stdout_path = Path::new(process.stdout_path.as_deref().unwrap());
            let command_path = Path::new(process.stdin_path.as_deref().unwrap());
            assert!(stdout_path.is_file());
            assert!(command_path.is_file());

            assert!(supervisor.recover()?.is_empty());
            assert!(stdout_path.is_file());
            assert!(command_path.is_file());
            assert!(matches!(
                supervisor.inspect(&process.id),
                Err(RefineError::NotFound(_))
            ));
            assert!(stdout_path.is_file());

            let process_path = supervisor
                .process_history_dir()
                .join(format!("{}.json", process.id));
            let reconciled: ManagedProcess =
                serde_json::from_slice(&fs::read(&process_path).unwrap()).unwrap();
            assert_eq!(reconciled.state, "exited");
            Ok(())
        },
    )
    .unwrap();

    assert!(result.output.contains("transcript survives recovery"));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    assert!(
        !supervisor
            .processes_dir()
            .join(format!("{}.json", result.process_id))
            .exists()
    );
    assert!(
        !supervisor
            .processes_dir()
            .join(format!("{}.stdout.log", result.process_id))
            .exists()
    );
    assert!(
        !supervisor
            .artifact_handoff_path(&result.process_id)
            .exists()
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn workflow_goal_agent_pty_uses_configured_final_environment_for_file_transport() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-final-environment");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let config_root = root.join("config");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::create_dir_all(config_root.join("refine")).unwrap();
    let mut configured = (0..23)
        .map(|index| format!("REFINE_CONFIGURED_{index}={}\n", "e".repeat(65_800)))
        .collect::<String>();
    configured.push_str("REFINE_SESSION_ROLE=configured-must-lose\n");
    configured.push_str("OPENAI_API_KEY=configured-must-be-removed\n");
    fs::write(config_root.join("refine/agent.env"), configured).unwrap();
    fs::write(
        &provider,
        concat!(
            "#!/bin/sh\n",
            "prompt_path=$(printf '%s' \"$1\" | sed -n '4{s/^`//;s/`$//;p;}')\n",
            "expected_bytes=$(printf '%s' \"$1\" | sed -n '7{s/^- UTF-8 bytes: `//;s/`$//;p;}')\n",
            "expected_sha=$(printf '%s' \"$1\" | sed -n '8{s/^- SHA-256: `//;s/`$//;p;}')\n",
            "test \"$(wc -c < \"$prompt_path\")\" = \"$expected_bytes\" || exit 2\n",
            "test \"$(sha256sum \"$prompt_path\" | cut -d' ' -f1)\" = \"$expected_sha\" || exit 3\n",
            "grep -q 'GOAL_FINAL_ENV_PROMPT_SECRET' \"$prompt_path\" || exit 4\n",
            "test \"$(printf '%s' \"$REFINE_CONFIGURED_0\" | wc -c)\" -eq 65800 || exit 5\n",
            "test \"$REFINE_SESSION_ROLE\" = goal || exit 6\n",
            "test \"${OPENAI_API_KEY-unset}\" = unset || exit 7\n",
            "printf 'goal-final-environment-prompt-received\\n'\n",
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");
    let previous_config = std::env::var_os("XDG_CONFIG_HOME");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
        std::env::set_var("XDG_CONFIG_HOME", &config_root);
    }
    let secret = "GOAL_FINAL_ENV_PROMPT_SECRET";
    let prompt = format!("{secret}{}", "p".repeat(60_000 - secret.len()));
    let result = run_goal_agent(
        GoalAgentLaunch {
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt,
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-FINAL-ENV"))]),
            completion_timeout: None,
        },
        |_| {},
    )
    .unwrap();

    assert!(
        result
            .output
            .contains("goal-final-environment-prompt-received")
    );
    unsafe {
        match previous_provider {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
        match previous_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn workflow_goal_agent_early_exec_failure_preserves_errno_and_cleans_channels() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-early-exec");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(&provider, "#!/definitely/missing/refine-interpreter\n").unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let error = run_goal_agent(
        GoalAgentLaunch {
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "large launch failure ".to_string() + &"x".repeat(158_078),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-EXEC-FAIL"))]),
            completion_timeout: None,
        },
        |_| {},
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("No such file") || message.contains("os error 2"),
        "{message}"
    );
    assert!(!message.contains("commands.jsonl"), "{message}");
    let process_dir = runtime_root.join("processes");
    assert!(
        !process_dir.exists()
            || fs::read_dir(&process_dir).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("commands"))
    );
    let prompts = runtime_root.join("agent-prompts");
    assert!(!prompts.exists() || fs::read_dir(prompts).unwrap().next().is_none());

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn silent_goal_agent_remains_autonomous_without_requesting_input() {
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-idle-attention");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\nsleep 0.2\nprintf 'made-the-best-decision\\n'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let mut metadata = Map::new();
    metadata.insert("goal_id".to_string(), json!("GOAL3"));
    let mut attention = Vec::new();
    let result = run_goal_agent(
        GoalAgentLaunch {
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test".to_string(),
            metadata,
            completion_timeout: None,
        },
        |request| attention.push(request),
    )
    .unwrap();
    assert!(attention.is_empty());
    assert!(result.output.contains("made-the-best-decision"));

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(root).unwrap();
}
