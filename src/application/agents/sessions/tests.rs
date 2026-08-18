use super::*;
use crate::infrastructure::process::subprocess::ProcessSupervisor;
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
    assert!(prompt.contains("completed|no_change_needed|deviated|rejected|blocked"));
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
fn every_goal_agent_phase_uses_the_integer_guidance_completion_contract() {
    let signal = Path::new("/runtime/processes/goal-agent.signal.json");
    let plan = goal_agent_protocol_prompt("PLAN_SENTINEL", signal, Some("plan"));
    let criticize = goal_agent_protocol_prompt("CRITICIZE_SENTINEL", signal, Some("criticize"));
    let revise = goal_agent_protocol_prompt("REVISE_SENTINEL", signal, Some("revise"));
    let implement = goal_agent_protocol_prompt("IMPLEMENT_SENTINEL", signal, Some("implement"));

    for prompt in [&plan, &criticize, &revise, &implement] {
        assert!(prompt.contains("zero-based integer Completion Index"));
        assert!(prompt.contains("never use candidate names or stable configuration IDs"));
        assert!(prompt.contains("\"guidance_applied\":[0]"));
        assert!(prompt.contains(signal.to_str().unwrap()));
        assert!(!prompt.contains("{{"));
    }
    for prompt in [&plan, &criticize, &revise] {
        assert!(prompt.contains("required `planning_result`"));
        assert!(prompt.contains("never omitted or quoted as a string"));
        assert!(!prompt.contains("implementation_evidence"));
    }
    assert!(plan.contains("\"checklist\""));
    assert!(criticize.contains("\"findings\""));
    assert!(revise.contains("\"criticism_resolutions\""));
    assert!(revise.contains("\"criticism_id\":\"C1\""));
    assert!(revise.contains("\"resolution\":\"how the revised plan resolves"));
    assert!(implement.contains("implementation_evidence"));
    assert!(!implement.contains("planning_result"));
}

#[test]
fn completion_signal_rejects_stable_guidance_ids_as_a_schema_error() {
    let root = unique_temp_dir("goal-agent-invalid-guidance-signal");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent.signal.json");
    fs::write(
        &signal_path,
        r#"{"state":"completed","message":"planned","guidance_applied":["guidance-1"],"planning_result":{"summary":"Plan summary.","checklist":[]}}"#,
    )
    .unwrap();

    let read = SignalReader::default().take(&signal_path).unwrap();
    let SignalRead::InvalidContract(diagnostic) = read else {
        panic!("schema-invalid signal must be rejected");
    };

    assert!(diagnostic.contains("does not match the required schema"));
    assert!(diagnostic.contains("guidance_applied[0]"));
    assert!(diagnostic.contains("expected usize"));
    assert!(signal_path.is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_signal_accepts_zero_based_guidance_indexes() {
    let root = unique_temp_dir("goal-agent-valid-guidance-signal");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent.signal.json");
    fs::write(
        &signal_path,
        r#"{"state":"completed","message":"planned","guidance_applied":[0,2],"planning_result":{"summary":"Plan summary.","checklist":[]}}"#,
    )
    .unwrap();

    let SignalRead::Valid(signal) = SignalReader::default().take(&signal_path).unwrap() else {
        panic!("integer Guidance indexes must satisfy the completion schema");
    };
    assert_eq!(signal.guidance_applied, Some(vec![0, 2]));
    assert!(!signal_path.exists());
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
    let SignalRead::MalformedTransport(diagnostic) = reader.take(&signal_path).unwrap() else {
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
    let SignalRead::InvalidContract(diagnostic) = reader.take(&signal_path).unwrap() else {
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

#[test]
fn implementation_completion_signal_accepts_no_change_needed_evidence() {
    let root = unique_temp_dir("goal-agent-no-change-needed-signal");
    fs::create_dir_all(&root).unwrap();
    let signal_path = root.join("goal-agent.signal.json");
    fs::write(
        &signal_path,
        r#"{"state":"completed","message":"nothing to change","guidance_applied":[],"implementation_evidence":{"checklist":[{"id":"P1","outcome":"no_change_needed","evidence":"The requested behavior is already present."}],"verification":["cargo test --lib: passed"]}}"#,
    )
    .unwrap();

    let SignalRead::Valid(signal) = SignalReader::default().take(&signal_path).unwrap() else {
        panic!("no_change_needed must be accepted by the Goal Agent completion schema");
    };
    let implementation_evidence = signal.implementation_evidence.unwrap();
    let outcome = &implementation_evidence.checklist.first().unwrap().outcome;
    assert_eq!(
        outcome,
        &crate::model::goal::ImplementationChecklistOutcome::NoChangeNeeded
    );
    assert!(!signal_path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn planning_session_returns_the_structured_result_from_its_completion_signal() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-planning-result");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\nprintf '%s\\n' '{\"state\":\"completed\",\"message\":\"planned\",\"guidance_applied\":[0],\"planning_result\":{\"summary\":\"Change the boundary so debug workers use the active executable.\",\"checklist\":[{\"id\":\"P1\",\"description\":\"Pass the active executable to supervised workers.\"}]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\nsleep 10\n",
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
            provider_session: None,
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "plan".to_string(),
            metadata,
            completion_timeout: None,
            idle_timeout: None,
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(result.output, "planned");
    assert_eq!(result.guidance_applied, Some(vec![0]));
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
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
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
            provider_session: None,
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test timeout".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-TIMEOUT"))]),
            completion_timeout: Some(Duration::from_millis(200)),
            idle_timeout: None,
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

#[cfg(unix)]
#[test]
fn goal_agent_idle_timeout_fails_fast_and_preserves_the_transcript() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-idle-timeout");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    // Produces output once, then stalls silently: the shape of a hung agent
    // that previously burned the whole completion cap with no evidence left.
    fs::write(&provider, "#!/bin/sh\necho still-alive-once\nsleep 10\n").unwrap();
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
            provider_session: None,
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test idle watchdog".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-IDLE"))]),
            completion_timeout: Some(Duration::from_secs(30)),
            idle_timeout: Some(Duration::from_millis(300)),
        },
        |_| {},
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(
        message.contains("produced no output for"),
        "expected an idle-timeout failure, got: {message}"
    );
    assert!(
        message.contains("transcript preserved at"),
        "expected the failure to point at preserved evidence, got: {message}"
    );
    // Failed fast on the idle budget, nowhere near the completion cap.
    assert!(started_at.elapsed() < Duration::from_secs(10));
    let preserved = fs::read_dir(runtime_root.join("processes"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".stdout.log.failed"))
        })
        .expect("preserved transcript missing");
    assert!(
        fs::read_to_string(&preserved)
            .unwrap()
            .contains("still-alive-once"),
        "preserved transcript lost the agent's output"
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
fn goal_agent_survives_a_deleted_command_channel() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-missing-commands");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    // Delete the attach channel mid-run — the reaper race that killed whole
    // sessions — then complete normally. The channel is a convenience input
    // path for humans; losing it must not fail an autonomous run.
    fs::write(
        &provider,
        "#!/bin/sh\nrm -f \"${REFINE_AGENT_SIGNAL_PATH%.signal.json}.commands.jsonl\"\nsleep 0.3\nprintf '%s\\n' '{\"state\":\"completed\",\"message\":\"survived\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\nsleep 10\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let result = run_goal_agent(
        GoalAgentLaunch {
            provider_session: None,
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test deleted command channel".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-NOCMD"))]),
            completion_timeout: Some(Duration::from_secs(30)),
            idle_timeout: None,
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(result.output, "survived");

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
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
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
                provider_session: None,
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "test".to_string(),
                metadata,
                completion_timeout: None,
                idle_timeout: None,
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
            provider_session: None,
            runtime_root: runtime_root.clone(),
            cwd: app_root.clone(),
            provider: "smoke-ai".to_string(),
            prompt: "duplicate".to_string(),
            metadata: duplicate_metadata,
            completion_timeout: None,
            idle_timeout: None,
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
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
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
                provider_session: None,
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "test".to_string(),
                metadata,
                completion_timeout: None,
                idle_timeout: None,
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
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
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
            provider_session: None,
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test".to_string(),
            metadata,
            completion_timeout: None,
            idle_timeout: None,
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
fn workflow_goal_agent_pty_delivers_oversized_prompts_by_file_with_final_environment() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-final-environment");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
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
            "test \"$REFINE_SESSION_ROLE\" = goal || exit 6\n",
            "printf 'goal-final-environment-prompt-received\\n'\n",
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    // Past the inline transport cap, so the prompt must arrive as a verified
    // file even before argv and environment budgets come into play.
    let secret = "GOAL_FINAL_ENV_PROMPT_SECRET";
    let prompt = format!("{secret}{}", "p".repeat(80_000 - secret.len()));
    let result = run_goal_agent(
        GoalAgentLaunch {
            provider_session: None,
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt,
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-FINAL-ENV"))]),
            completion_timeout: None,
            idle_timeout: None,
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
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn workflow_goal_agent_early_exec_failure_preserves_errno_and_cleans_channels() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
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
            provider_session: None,
            runtime_root: runtime_root.clone(),
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "large launch failure ".to_string() + &"x".repeat(158_078),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-EXEC-FAIL"))]),
            completion_timeout: None,
            idle_timeout: None,
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

/// Scripted PTY stand-in for the reader loop: plays back the given reads, then
/// raises `child_exited` and reads zero forever, the shape of a child that was
/// reaped after its last output.
struct ScriptedPty {
    steps: std::collections::VecDeque<std::io::Result<Vec<u8>>>,
    child_exited: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Read for ScriptedPty {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.steps.pop_front() {
            Some(Ok(bytes)) => {
                buf[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
            Some(Err(error)) => Err(error),
            None => {
                self.child_exited
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(0)
            }
        }
    }
}

/// Drives `pump_pty_output` over a scripted read sequence and reports the pump
/// result, the captured transcript, and whether the activity clock advanced.
fn pump_scripted_pty(steps: Vec<std::io::Result<Vec<u8>>>) -> (RefineResult<()>, String, bool) {
    let dir = unique_temp_dir("pty-pump");
    fs::create_dir_all(&dir).unwrap();
    let transcript_path = dir.join("transcript.log");
    let mut transcript = fs::File::create(&transcript_path).unwrap();
    let child_exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut reader = ScriptedPty {
        steps: steps.into(),
        child_exited: std::sync::Arc::clone(&child_exited),
    };
    let started = std::time::Instant::now();
    let activity = std::sync::Mutex::new(started);
    let result = pump_pty_output(
        &mut reader,
        &mut transcript,
        &transcript_path,
        &activity,
        &child_exited,
    );
    let advanced = *activity.lock().unwrap() > started;
    let contents = fs::read_to_string(&transcript_path).unwrap();
    fs::remove_dir_all(dir).unwrap();
    (result, contents, advanced)
}

#[test]
fn pty_pump_retries_zero_reads_until_the_child_is_confirmed_gone() {
    // Output arrives only after several zero-byte reads: treating the first
    // Ok(0) as EOF (the production incident) would end the pump with an empty
    // transcript instead of capturing it.
    let (result, transcript, advanced) = pump_scripted_pty(vec![
        Ok(Vec::new()),
        Ok(Vec::new()),
        Ok(Vec::new()),
        Ok(b"alive-after-transient-eio".to_vec()),
        Ok(Vec::new()),
    ]);

    result.unwrap();
    assert_eq!(transcript, "alive-after-transient-eio");
    assert!(advanced, "PTY output must reset the activity clock");
}

#[test]
fn pty_pump_treats_a_zero_read_as_eof_once_the_child_is_reaped() {
    let (result, transcript, advanced) = pump_scripted_pty(Vec::new());

    result.unwrap();
    assert_eq!(transcript, "");
    assert!(!advanced);
}

#[test]
fn pty_pump_retries_interrupted_reads() {
    let (result, transcript, _) = pump_scripted_pty(vec![
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "interrupted",
        )),
        Ok(b"resumed".to_vec()),
    ]);

    result.unwrap();
    assert_eq!(transcript, "resumed");
}

#[test]
fn pty_pump_propagates_hard_read_errors() {
    let (result, transcript, _) = pump_scripted_pty(vec![Err(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "master torn down",
    ))]);

    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("Goal Agent output stream failed"),
        "{message}"
    );
    assert!(message.contains("master torn down"), "{message}");
    assert_eq!(transcript, "");
}

#[test]
fn reader_death_while_the_agent_runs_names_capture_failure_not_idleness() {
    let failed = transcript_capture_failure(Ok(Err(RefineError::Io(
        "Goal Agent output stream failed: boom".to_string(),
    ))))
    .to_string();
    assert!(
        failed.contains("transcript capture failed while the agent was still running"),
        "{failed}"
    );
    assert!(failed.contains("boom"), "{failed}");
    assert!(!failed.contains("produced no output"), "{failed}");

    let silent = transcript_capture_failure(Ok(Ok(()))).to_string();
    assert!(
        silent.contains("transcript reader stopped without an error"),
        "{silent}"
    );
    assert!(!silent.contains("produced no output"), "{silent}");
}

#[cfg(unix)]
#[test]
fn goal_agent_survives_transient_pty_eio_while_writing_its_signal_slowly() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-eio-liveness");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    // The production incident's shape: the agent drops every PTY fd (making
    // master reads return EIO while it keeps running), then works quietly,
    // its only observable progress being chunked signal-file writes spread
    // over more than three idle budgets. Pre-fix the reader thread died on
    // the first EIO, froze the activity clock, and the watchdog killed a
    // live agent.
    fs::write(
        &provider,
        concat!(
            "#!/bin/sh\n",
            "echo booted\n",
            "exec </dev/null >/dev/null 2>&1\n",
            "sig=\"$REFINE_AGENT_SIGNAL_PATH\"\n",
            "printf '%s' '{\"state\":\"comp' > \"$sig\"\n",
            "sleep 0.3\n",
            "printf '%s' 'leted\",' >> \"$sig\"\n",
            "sleep 0.3\n",
            "printf '%s' '\"message\"' >> \"$sig\"\n",
            "sleep 0.3\n",
            "printf '%s' ':\"surv' >> \"$sig\"\n",
            "sleep 0.3\n",
            "printf '%s' 'ived-' >> \"$sig\"\n",
            "sleep 0.3\n",
            "printf '%s' 'eio\"' >> \"$sig\"\n",
            "sleep 0.3\n",
            "printf '%s' '}' >> \"$sig\"\n",
            "sleep 10\n",
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let result = run_goal_agent(
        GoalAgentLaunch {
            provider_session: None,
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test EIO liveness".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-EIO"))]),
            completion_timeout: Some(Duration::from_secs(30)),
            idle_timeout: Some(Duration::from_millis(500)),
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(result.output, "survived-eio");

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
fn goal_agent_ansi_only_output_keeps_the_session_alive() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-ansi-liveness");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    // A spinner repainting one cell — carriage return, erase-line, one glyph —
    // is real PTY activity even though it never adds a printable line. The
    // loop spans more than two idle budgets before completing.
    fs::write(
        &provider,
        concat!(
            "#!/bin/sh\n",
            "i=0\n",
            "while [ \"$i\" -lt 12 ]; do\n",
            "  printf '\\r\u{1b}[K|'\n",
            "  sleep 0.1\n",
            "  i=$((i+1))\n",
            "done\n",
            "printf '%s\\n' '{\"state\":\"completed\",\"message\":\"ansi-alive\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n",
            "sleep 10\n",
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let result = run_goal_agent(
        GoalAgentLaunch {
            provider_session: None,
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test ANSI liveness".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-ANSI"))]),
            completion_timeout: Some(Duration::from_secs(30)),
            idle_timeout: Some(Duration::from_millis(500)),
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(result.output, "ansi-alive");

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
fn goal_agent_idle_kill_takes_the_whole_process_group_down() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-group-kill");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    // The background descendant inherits the PTY slave. Killing only the
    // session leader would leave it holding the terminal, and joining the
    // reader would block on its 30-second timer instead of the idle budget.
    fs::write(&provider, "#!/bin/sh\necho started\nsleep 30 &\nsleep 30\n").unwrap();
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
            provider_session: None,
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test group kill".to_string(),
            metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-GROUP-KILL"))]),
            completion_timeout: Some(Duration::from_secs(60)),
            idle_timeout: Some(Duration::from_millis(400)),
        },
        |_| {},
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("produced no output for"), "{message}");
    assert!(message.contains("transcript preserved at"), "{message}");
    // Idle budget plus kill/join/preserve overhead — nowhere near the
    // descendant's 30-second timer that a leader-only kill would wait out.
    assert!(
        started_at.elapsed() < Duration::from_secs(5),
        "idle kill waited on the agent's descendants: {:?}",
        started_at.elapsed()
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
fn silent_goal_agent_remains_autonomous_without_requesting_input() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
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
            provider_session: None,
            runtime_root,
            cwd: app_root,
            provider: "smoke-ai".to_string(),
            prompt: "test".to_string(),
            metadata,
            completion_timeout: None,
            idle_timeout: None,
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
