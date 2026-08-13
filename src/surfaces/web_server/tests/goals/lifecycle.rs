use super::*;
use sha2::{Digest, Sha256};

use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationPlan, ImplementationPlanBinding,
    ImplementationPlanPhase, ImplementationPlanState,
};

#[test]
fn web_server_transitions_goal_and_refine_dir() {
    let temp_root = unique_temp_dir("http-transition");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "HTTP transition",
              "status": "backlog",
              "priority": "low",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();
    let projection = FileProjectProjectionStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let mut server = server_with_projection();
    server.projection = projection;
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals/GOAL1/transition".to_string(),
        body: Some(json!({"status": "todo"})),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["goal"]["status"], "todo");
    assert!(
        fs::read_to_string(goal_dir.join("goal.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    let patch_response = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: Some(json!({"status": "backlog"})),
    });
    assert_eq!(patch_response.status, 200);
    assert_eq!(patch_response.body["goal"]["status"], "backlog");

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_open_agent_attaches_to_the_workflow_goal_agent() {
    let _env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp_root = unique_temp_dir("http-goal-agent-session");
    let app_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8082");
    let provider = temp_root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\nprintf 'goal-agent-ready\\n'\nread answer\nprintf 'goal-agent-answer:%s\\n' \"$answer\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
    }
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let runtime_for_thread = runtime_root.clone();
    let app_for_thread = app_root.clone();
    let session_thread = thread::spawn(move || {
        let mut metadata = serde_json::Map::new();
        metadata.insert("goal_id".to_string(), json!("GOAL1"));
        run_goal_agent(
            GoalAgentLaunch {
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "Implement Goal GOAL1".to_string(),
                metadata,
            },
            |_| {},
        )
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while FileProcessSupervisor::new(&runtime_root)
        .list()
        .unwrap()
        .is_empty()
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());
    let opened = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"profile": "goal", "goal_id": "GOAL1"})),
    });
    assert_eq!(opened.status, 200, "{}", opened.body);
    assert_eq!(opened.body["profile"], "goal");
    assert_eq!(opened.body["goal_id"], "GOAL1");
    let session_id = opened.body["id"].as_str().unwrap();
    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/input"),
        body: Some(json!({"data": "attached\r"})),
    });
    assert_eq!(input.status, 200, "{}", input.body);
    let result = session_thread.join().unwrap().unwrap();
    assert!(result.output.contains("goal-agent-answer:attached"));

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_opens_an_in_progress_goal_diagnostic_when_no_goal_agent_is_running() {
    let _env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp_root = unique_temp_dir("http-in-progress-goal-agent-registration");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let provider = temp_root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\ntrap 'exit 0' TERM INT\nprintf 'diagnostic-ready:%s\\n' \"$*\"\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
    }
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Workflow still owns this Goal", Some("GOAL-IN-PROGRESS"))
        .unwrap();
    work_items
        .append_goal_round_summary(
            "GOAL-IN-PROGRESS",
            "Workflow",
            "Wait for the workflow Agent to register",
        )
        .unwrap();
    work_items
        .transition_goal_status("GOAL-IN-PROGRESS", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-IN-PROGRESS", GoalStatus::Plan)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());
    let opened = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"profile": "goal", "goal_id": "GOAL-IN-PROGRESS"})),
    });

    assert_eq!(opened.status, 200, "{}", opened.body);
    let session_id = opened.body["id"].as_str().unwrap().to_string();
    let process_id = opened.body["process_id"].as_str().unwrap();
    let process = FileProcessSupervisor::new(&runtime_root)
        .inspect(process_id)
        .unwrap()
        .api_json();
    assert_eq!(process["attached_goal_id"], "GOAL-IN-PROGRESS");
    assert!(process.get("goal_id").is_none());
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-IN-PROGRESS")
            .unwrap()
            .goal
            .status,
        GoalStatus::Plan
    );

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_reports_between_planning_phases_without_launching_a_diagnostic_agent() {
    let temp_root = unique_temp_dir("http-goal-between-planning-phases");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    fs::create_dir_all(&app_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Between planning phases", Some("GOAL-BETWEEN-PHASES"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL-BETWEEN-PHASES", "Workflow", "Implement")
        .unwrap();
    work_items
        .transition_goal_status("GOAL-BETWEEN-PHASES", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-BETWEEN-PHASES", GoalStatus::Plan)
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL-BETWEEN-PHASES",
            "refine/GOAL-BETWEEN-PHASES/round-1",
            "main",
            "base123",
            None,
        )
        .unwrap();
    let context = json!({
        "version": 1,
        "goal": {"id": "GOAL-BETWEEN-PHASES"},
        "current_round": {"round": 1, "prompt": "Implement"},
        "previous_rounds": []
    });
    work_items
        .update_goal_round_evaluation_summary(
            "GOAL-BETWEEN-PHASES",
            0,
            &json!({"agent_context": context.clone()}),
        )
        .unwrap();
    work_items
        .replace_goal_round_implementation_plan(
            "GOAL-BETWEEN-PHASES",
            0,
            None,
            &ImplementationPlan {
                schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
                state: ImplementationPlanState::InProgress,
                phase: ImplementationPlanPhase::Criticize,
                binding: ImplementationPlanBinding {
                    goal_id: "GOAL-BETWEEN-PHASES".to_string(),
                    round_idx: 0,
                    context_version: 1,
                    context_digest: format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(&context).unwrap())
                    ),
                    implementation_branch: "refine/GOAL-BETWEEN-PHASES/round-1".to_string(),
                    target_branch: "main".to_string(),
                    base_commit: "base123".to_string(),
                },
                started_at: "2026-08-11T10:00:00Z".to_string(),
                phase_started_at: "2026-08-11T10:01:00Z".to_string(),
                updated_at: "2026-08-11T10:01:00Z".to_string(),
                completed_at: None,
                proposal: None,
                criticism: None,
                final_plan: None,
                implementation: None,
                failure: None,
            },
        )
        .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());
    let opened = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"profile": "goal", "goal_id": "GOAL-BETWEEN-PHASES"})),
    });

    assert_eq!(opened.status, 409, "{}", opened.body);
    assert!(
        opened
            .body
            .to_string()
            .contains("between supervised implementation-planning phase processes")
    );
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .is_empty()
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_opens_failed_goal_in_diagnostic_session_without_workflow_mutation() {
    let _env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp_root = unique_temp_dir("http-failed-goal-diagnostic-agent");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let provider = temp_root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\ntrap 'exit 0' TERM INT\nprintf 'diagnostic-ready:%s\\n' \"$*\"\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
    }
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Diagnose production failure", Some("GOAL-DIAGNOSTIC"))
        .unwrap();
    work_items
        .append_goal_round_summary(
            "GOAL-DIAGNOSTIC",
            "Production User",
            "Investigate the failed request",
        )
        .unwrap();
    work_items
        .update_latest_goal_round_evaluation_summary(
            "GOAL-DIAGNOSTIC",
            &json!({
                "failure_category": "provider",
                "failure_message": "Agent authentication expired",
                "failure_at": "2026-08-04T10:00:00Z"
            }),
        )
        .unwrap();
    work_items
        .set_goal_status_unchecked("GOAL-DIAGNOSTIC", &GoalStatus::Failed)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());
    let opened = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({
            "profile": "goal",
            "goal_id": "GOAL-DIAGNOSTIC",
            "cols": 100,
            "rows": 30
        })),
    });
    assert_eq!(opened.status, 200, "{}", opened.body);
    assert_eq!(opened.body["profile"], "goal");
    assert_eq!(opened.body["goal_id"], "GOAL-DIAGNOSTIC");
    let session_id = opened.body["id"].as_str().unwrap().to_string();
    let process_id = opened.body["process_id"].as_str().unwrap();
    let process = FileProcessSupervisor::new(&runtime_root)
        .inspect(process_id)
        .unwrap()
        .api_json();
    assert_eq!(process["attached_goal_id"], "GOAL-DIAGNOSTIC");
    assert!(process.get("goal_id").is_none());

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut transcript = String::new();
    while Instant::now() < deadline {
        transcript = crate::surfaces::web_server::support::terminal_events_since(
            &runtime_root,
            &session_id,
            0,
        )
        .unwrap()
        .iter()
        .filter_map(|event| event.get("data").and_then(serde_json::Value::as_str))
        .collect();
        if transcript.contains("diagnostic-ready:") {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        transcript.contains("Agent authentication expired"),
        "{transcript}"
    );
    assert!(
        transcript.contains("This is a diagnostic session"),
        "{transcript}"
    );

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.body["ok"], true);
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-DIAGNOSTIC")
            .unwrap()
            .goal
            .status,
        GoalStatus::Failed
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    remove_temp_dir(&temp_root);
}

#[test]
fn browser_terminal_stop_fails_the_goal_after_stopping_its_local_agent() {
    let _env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp_root = unique_temp_dir("http-goal-agent-terminal-stop");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let provider = temp_root.join("smoke-ai");
    fs::create_dir_all(&app_root).unwrap();
    fs::write(
        &provider,
        "#!/bin/sh\ntrap 'exit 0' TERM INT\nprintf 'goal-agent-ready\\n'\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
    }
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Stop workflow Goal Agent", Some("GOAL-TERMINAL-STOP"))
        .unwrap();
    work_items
        .append_goal_round_summary(
            "GOAL-TERMINAL-STOP",
            "Browser test",
            "Stop through the Goal terminal",
        )
        .unwrap();
    work_items
        .transition_goal_status("GOAL-TERMINAL-STOP", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-TERMINAL-STOP", GoalStatus::Plan)
        .unwrap();
    let runtime_for_thread = runtime_root.clone();
    let app_for_thread = app_root.clone();
    let session_thread = thread::spawn(move || {
        let mut metadata = serde_json::Map::new();
        metadata.insert("goal_id".to_string(), json!("GOAL-TERMINAL-STOP"));
        metadata.insert("kind".to_string(), json!("workflow"));
        metadata.insert("node_id".to_string(), json!("default"));
        metadata.insert("provider".to_string(), json!("smoke-ai"));
        metadata.insert(
            "target_app_id".to_string(),
            json!(app_for_thread.display().to_string()),
        );
        metadata.insert("round_idx".to_string(), json!(0));
        metadata.insert("workflow_state".to_string(), json!("in-progress"));
        run_goal_agent(
            GoalAgentLaunch {
                runtime_root: runtime_for_thread,
                cwd: app_for_thread,
                provider: "smoke-ai".to_string(),
                prompt: "Implement Goal GOAL-TERMINAL-STOP".to_string(),
                metadata,
            },
            |_| {},
        )
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while FileProcessSupervisor::new(&runtime_root)
        .list()
        .unwrap()
        .is_empty()
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());
    let opened = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({
            "profile": "goal",
            "goal_id": "GOAL-TERMINAL-STOP"
        })),
    });
    assert_eq!(opened.status, 200, "{}", opened.body);
    let session_id = opened.body["id"].as_str().unwrap().to_string();
    let process_id = opened.body["process_id"].as_str().unwrap().to_string();

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.body["stopped"], true);
    assert_eq!(stopped.body["process"]["id"], process_id);
    assert_eq!(stopped.body["termination"]["confirmed_exit"], true);
    assert_eq!(stopped.body["goal"]["id"], "GOAL-TERMINAL-STOP");
    assert_eq!(stopped.body["goal"]["status"], "failed");
    assert_eq!(stopped.body["worktrees_retained"], true);
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .inspect(&process_id)
            .is_err()
    );
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-TERMINAL-STOP")
            .unwrap()
            .goal
            .status,
        GoalStatus::Failed
    );
    let receipt_path = fs::read_dir(runtime_root.join("process-stop-outcomes"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("process-{process_id}-"))
        })
        .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["termination"]["confirmed_exit"], true);
    assert_eq!(receipt["goal_failed"], true);
    assert_eq!(receipt["goal_requeued"], false);
    assert_eq!(receipt["worktrees_retained"], true);
    let _ = session_thread.join().unwrap();

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    remove_temp_dir(&temp_root);
}

#[test]
fn public_goal_cancel_api_reports_goal_without_a_local_process_as_durably_cancelled() {
    let temp_root = unique_temp_dir("http-goal-cancel-no-process");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let goal_id = "GOAL-API-CANCEL-NO-PROCESS";
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Cancel managed Goal", Some(goal_id))
        .unwrap();
    work_items
        .transition_goal_status(goal_id, GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status(goal_id, GoalStatus::Plan)
        .unwrap();
    let (command, args) = if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 300 127.0.0.1 >NUL".to_string()],
        )
    } else {
        ("sleep".to_string(), vec!["300".to_string()])
    };
    let process = FileProcessSupervisor::new(runtime_root.join("agents"))
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command,
            args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::Map::from_iter([
                ("kind".to_string(), json!("interactive_session")),
                ("provider".to_string(), json!("smoke-ai")),
                ("goal_id".to_string(), json!(goal_id)),
            ]),
        })
        .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());
    let cancelled = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/goals/{goal_id}/cancel"),
        body: None,
    });

    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    assert_eq!(cancelled.body["cancelled"], true);
    assert_eq!(cancelled.body["goal"]["status"], "cancelled");
    assert_eq!(cancelled.body["worktrees_retained"], true);
    assert!(
        cancelled.body["process_failures"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        work_items.show_goal_summary(goal_id).unwrap().goal.status,
        GoalStatus::Cancelled
    );
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    remove_temp_dir(&temp_root);
}

#[test]
fn public_process_stop_api_does_not_requeue_already_cancelled_goal() {
    let temp_root = unique_temp_dir("http-process-stop-cancelled-no-process");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let goal_id = "GOAL-API-STOP-CANCELLED";
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Stop cancelled managed Goal", Some(goal_id))
        .unwrap();
    work_items.cancel_goal_summary(goal_id).unwrap();
    let process = FileProcessSupervisor::new(runtime_root.join("agents"))
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: if cfg!(windows) { "cmd" } else { "sleep" }.to_string(),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "ping -n 300 127.0.0.1 >NUL".to_string()]
            } else {
                vec!["300".to_string()]
            },
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::Map::from_iter([
                ("kind".to_string(), json!("interactive_session")),
                ("provider".to_string(), json!("smoke-ai")),
                ("goal_id".to_string(), json!(goal_id)),
            ]),
        })
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root.clone());

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/processes/{}/stop", process.id),
        body: Some(json!({"signal": "terminate"})),
    });

    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.body["stopped"], true);
    assert_eq!(stopped.body["goal"]["status"], "cancelled");
    assert_eq!(stopped.body["goal_requeued"], false);
    assert_eq!(stopped.body["worktrees_retained"], true);
    assert_eq!(
        work_items.show_goal_summary(goal_id).unwrap().goal.status,
        GoalStatus::Cancelled
    );
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());

    remove_temp_dir(&temp_root);
}
