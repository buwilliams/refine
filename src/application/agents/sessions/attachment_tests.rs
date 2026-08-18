use super::*;

use std::os::unix::fs::PermissionsExt;

fn unique_temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{name}-{}", Uuid::new_v4()))
}

#[cfg(unix)]
#[test]
fn toolbar_attachment_wins_a_deadline_race_then_completes() {
    let _env_guard = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = unique_temp_dir("goal-agent-toolbar-protection");
    let runtime_root = root.join("run/8082");
    let app_root = root.join("app");
    let provider = root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\nprintf 'ready\\n'\nread answer\nprintf '%s\\n' '{\"state\":\"completed\",\"message\":\"toolbar-completed\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\nsleep 10\n",
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
        run_goal_agent(
            GoalAgentLaunch {
                provider_session: None,
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "test Toolbar timeout protection".to_string(),
                metadata: Map::from_iter([("goal_id".to_string(), json!("GOAL-TOOLBAR"))]),
                completion_timeout: Some(Duration::from_millis(800)),
                idle_timeout: Some(Duration::from_millis(800)),
            },
            |_| {},
        )
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        if let Ok(snapshot) = find_goal_agent_session(&runtime_root, "GOAL-TOOLBAR") {
            break snapshot;
        }
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    };
    assert!(!snapshot.toolbar_timeout_protected);
    // Put both watchdogs near their deadline. The runtime must consume and
    // persist the queued attachment before evaluating them in its next poll.
    thread::sleep(Duration::from_millis(650));
    let attached = attach_toolbar_goal_agent_session(&runtime_root, &snapshot.id).unwrap();
    assert_eq!(attached.id, snapshot.id);
    assert_eq!(attached.process_id, snapshot.process_id);
    assert!(attached.toolbar_timeout_protected);
    let reattached = attach_toolbar_goal_agent_session(&runtime_root, &snapshot.id).unwrap();
    assert!(reattached.toolbar_timeout_protected);
    let (_, protected_metadata) = session_process(&runtime_root, &snapshot.id).unwrap();
    assert_eq!(
        protected_metadata[TOOLBAR_ATTACHMENT_ACKS_KEY]
            .as_array()
            .unwrap()
            .len(),
        2,
        "each Toolbar open must receive its own runtime acknowledgment"
    );

    thread::sleep(Duration::from_millis(300));
    assert!(
        find_agent_session(&runtime_root, &snapshot.id)
            .unwrap()
            .alive,
        "an acknowledged attachment must survive both elapsed watchdogs"
    );
    send_agent_session_input(&runtime_root, &snapshot.id, "finish\r").unwrap();
    let result = run.join().unwrap().unwrap();
    assert_eq!(result.output, "toolbar-completed");

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
fn toolbar_attachment_rejects_non_goal_sessions_without_writing_a_command() {
    let root = unique_temp_dir("toolbar-attachment-profile-isolation");
    let runtime_root = root.join("run/8082");
    let command_path = runtime_root.join("processes/agent.commands.jsonl");
    fs::create_dir_all(command_path.parent().unwrap()).unwrap();
    fs::write(&command_path, "").unwrap();
    let session_id = Uuid::new_v4().to_string();
    let metadata = Map::from_iter([
        ("kind".to_string(), json!("interactive_session")),
        ("profile".to_string(), json!("agent")),
        ("session_id".to_string(), json!(&session_id)),
        (
            "command_path".to_string(),
            json!(command_path.display().to_string()),
        ),
    ]);
    FileProcessSupervisor::new(&runtime_root)
        .register(ManagedProcess {
            id: "ordinary-agent".to_string(),
            owner: ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Agent".to_string()),
            details: Some(serde_json::to_string(&metadata).unwrap()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: Some(command_path.display().to_string()),
            limits: None,
            started_at: Utc::now().to_rfc3339(),
            exit_code: None,
        })
        .unwrap();

    let error = attach_toolbar_goal_agent_session(&runtime_root, &session_id).unwrap_err();
    assert!(matches!(error, RefineError::Conflict(_)));
    assert!(fs::read_to_string(&command_path).unwrap().is_empty());
    assert!(matches!(
        attach_toolbar_goal_agent_session(&runtime_root, "different-session"),
        Err(RefineError::Conflict(_))
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn toolbar_attachment_without_a_runtime_ack_never_reports_success() {
    let root = unique_temp_dir("toolbar-attachment-missing-ack");
    let runtime_root = root.join("run/8082");
    let command_path = runtime_root.join("processes/goal.commands.jsonl");
    fs::create_dir_all(command_path.parent().unwrap()).unwrap();
    fs::write(&command_path, "").unwrap();
    let session_id = Uuid::new_v4().to_string();
    let metadata = Map::from_iter([
        ("kind".to_string(), json!("interactive_session")),
        ("profile".to_string(), json!("goal")),
        ("session_id".to_string(), json!(&session_id)),
        (
            "command_path".to_string(),
            json!(command_path.display().to_string()),
        ),
        (TOOLBAR_TIMEOUT_PROTECTED_KEY.to_string(), json!(false)),
    ]);
    FileProcessSupervisor::new(&runtime_root)
        .register(ManagedProcess {
            id: "unresponsive-goal-agent".to_string(),
            owner: ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Goal Agent".to_string()),
            details: Some(serde_json::to_string(&metadata).unwrap()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: Some(command_path.display().to_string()),
            limits: None,
            started_at: Utc::now().to_rfc3339(),
            exit_code: None,
        })
        .unwrap();

    let error = attach_toolbar_goal_agent_session_with_timeout(
        &runtime_root,
        &session_id,
        Duration::from_millis(80),
    )
    .unwrap_err();
    assert!(matches!(error, RefineError::Degraded(_)));
    assert!(
        fs::read_to_string(&command_path)
            .unwrap()
            .contains("toolbar_attach")
    );
    assert!(
        !find_agent_session(&runtime_root, &session_id)
            .unwrap()
            .toolbar_timeout_protected
    );
    fs::remove_dir_all(root).unwrap();
}
