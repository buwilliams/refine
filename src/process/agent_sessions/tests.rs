use super::*;
use crate::process::subprocess::ProcessSupervisor;
use std::os::unix::fs::PermissionsExt;

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{name}-{}", Uuid::new_v4()))
}

#[test]
fn workflow_goal_agent_prompt_excludes_interactive_checkout_guidance() {
    let prompt = goal_agent_protocol_prompt(
        "Implement Goal GOAL1",
        Path::new("/runtime/processes/goal-agent.signal.json"),
    );

    assert!(!prompt.contains("Active Refine executable"));
    assert!(!prompt.contains("checkout-local `./r`"));
    assert!(prompt.contains("/runtime/processes/goal-agent.signal.json"));
    assert!(!prompt.contains("{{"));
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
        },
        |_| {},
        |supervisor, process| {
            assert!(!FileProcessSupervisor::process_is_alive(process)?);
            let stdout_path = Path::new(process.stdout_path.as_deref().unwrap());
            assert!(stdout_path.is_file());

            assert!(supervisor.recover()?.is_empty());
            assert!(stdout_path.is_file());
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
