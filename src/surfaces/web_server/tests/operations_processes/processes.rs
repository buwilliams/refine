use super::*;

#[test]
fn runtime_process_status_counts_only_current_agents() {
    let mut runtime = RuntimeProjection {
        supervisor: json!({"runner_reachable": true}).as_object().cloned(),
        ..RuntimeProjection::default()
    };
    runtime.processes = vec![
        json!({
            "id": "exited-agent",
            "kind": "agent",
            "status": "exited"
        })
        .as_object()
        .cloned()
        .unwrap(),
        json!({
            "id": "completed-agent",
            "kind": "agent",
            "status": "completed"
        })
        .as_object()
        .cloned()
        .unwrap(),
        json!({
            "id": "running-chat",
            "kind": "chat",
            "status": "running"
        })
        .as_object()
        .cloned()
        .unwrap(),
        json!({
            "id": "stopped-ui",
            "kind": "ui",
            "status": "stopped"
        })
        .as_object()
        .cloned()
        .unwrap(),
    ];

    let status = runtime_process_status_value(&runtime);
    assert_eq!(status["agent_count"], 0);
    assert_eq!(status["process_count"], 1);
    assert_eq!(status["running_process_count"], 1);

    let summary = runtime_process_summary_value(&runtime);
    let processes = summary["processes"].as_array().unwrap();
    assert_eq!(processes.len(), 1);
    assert!(
        processes
            .iter()
            .any(|process| process["id"] == "running-chat")
    );
    assert!(
        processes
            .iter()
            .all(|process| process["id"] != "stopped-ui")
    );
    assert!(
        !processes
            .iter()
            .any(|process| process["id"] == "exited-agent")
    );
    assert!(
        !processes
            .iter()
            .any(|process| process["id"] == "completed-agent")
    );
}

#[test]
fn diagnostics_cache_keeps_process_health_live_after_startup_warming() {
    let temp_root = unique_temp_dir("http-diagnostics-live-process-health");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    server.warm_diagnostics_cache().unwrap();
    let warmed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/diagnostics".to_string(),
        body: None,
    });
    assert_eq!(warmed.status, 200);
    assert_eq!(warmed.body["processes"]["process_count"], 0);
    assert_eq!(warmed.body["processes"]["runner_reachable"], false);
    let warmed_provider = warmed.body["provider"].clone();
    let warmed_doctor = warmed.body["doctor"].clone();

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    for (id, worker_kind) in [
        ("workflow-runner", "workflow"),
        ("git-sync-runner", "git-sync"),
    ] {
        supervisor
            .register(ManagedProcess {
                id: id.to_string(),
                owner: ProcessOwner::Runner,
                pid: Some(std::process::id()),
                state: "running".to_string(),
                label: Some(format!("{worker_kind} runner")),
                details: Some(json!({"kind": "runner", "worker_kind": worker_kind}).to_string()),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();
    }
    supervisor
        .register(ManagedProcess {
            id: "stale-runner".to_string(),
            owner: ProcessOwner::Runner,
            pid: Some(u32::MAX),
            state: "running".to_string(),
            label: Some("Stale runner".to_string()),
            details: Some(json!({"kind": "runner", "worker_kind": "stale"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    fs::write(
        supervisor.processes_dir().join("terminal-runner.json"),
        serde_json::to_vec_pretty(&ManagedProcess {
            id: "terminal-runner".to_string(),
            owner: ProcessOwner::Runner,
            pid: Some(std::process::id()),
            state: "completed".to_string(),
            label: Some("Terminal runner".to_string()),
            details: Some(json!({"kind": "runner", "worker_kind": "terminal"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap(),
    )
    .unwrap();
    supervisor.set_workflow_paused(true).unwrap();

    let live = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/diagnostics".to_string(),
        body: None,
    });
    assert_eq!(live.status, 200);
    assert_eq!(live.body["processes"]["runner_reachable"], true);
    assert_eq!(live.body["processes"]["process_count"], 2);
    assert_eq!(live.body["processes"]["running_process_count"], 2);
    assert_eq!(live.body["processes"]["background_processes_stopped"], true);
    assert_eq!(live.body["processes"]["agents_paused"], true);
    assert_eq!(live.body["processes"]["paused"], true);
    assert_eq!(live.body["processes"]["workflow_paused"], true);
    assert_eq!(live.body["processes"]["processes"], json!([]));
    assert_eq!(live.body["provider"], warmed_provider);
    assert_eq!(live.body["doctor"], warmed_doctor);
    assert!(
        !supervisor
            .processes_dir()
            .join("stale-runner.json")
            .exists()
    );
    assert!(
        !supervisor
            .processes_dir()
            .join("terminal-runner.json")
            .exists()
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_resolves_nested_agent_process_stream_stop_and_not_found() {
    let temp_root = unique_temp_dir("http-nested-agent-process");
    let runtime_root = temp_root.join("run/8080");
    let agent_supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let stdout_path = runtime_root.join("agents/processes/nested-agent.stdout.log");
    let stderr_path = runtime_root.join("agents/processes/nested-agent.stderr.log");
    fs::create_dir_all(stdout_path.parent().unwrap()).unwrap();
    fs::write(&stdout_path, "nested agent stdout\n").unwrap();
    fs::write(&stderr_path, "nested agent stderr\n").unwrap();
    agent_supervisor
        .register(ManagedProcess {
            id: "nested-agent".to_string(),
            owner: ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("Nested agent".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root);

    let stream = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes/nested-agent/stream".to_string(),
        body: None,
    });
    assert_eq!(stream.status, 200, "{}", stream.body);
    assert_eq!(stream.body["process_id"], "nested-agent");
    assert!(
        stream.body["output"]
            .as_str()
            .unwrap()
            .contains("nested agent stdout")
    );
    assert!(
        stream.body["output"]
            .as_str()
            .unwrap()
            .contains("nested agent stderr")
    );

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/nested-agent/stop".to_string(),
        body: Some(json!({"signal": "terminate"})),
    });
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.body["stopped"], true);
    assert_eq!(stopped.body["process"]["id"], "nested-agent");
    assert_eq!(stopped.body["process"]["status"], "stopped");
    assert!(agent_supervisor.inspect("nested-agent").is_err());

    for (method, path) in [
        ("GET", "/api/processes/nested-agent/stream"),
        ("POST", "/api/processes/nested-agent/stop"),
    ] {
        let missing = server.handle(ApiRequest {
            method: method.to_string(),
            path: path.to_string(),
            body: None,
        });
        assert_eq!(missing.status, 404, "{}", missing.body);
        assert_eq!(missing.body["error"]["code"], "not_found");
        assert!(
            missing.body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("nested-agent")
        );
    }

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_controls_background_worker_enablement_by_worker_kind() {
    let temp_root = unique_temp_dir("http-background-worker-control");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor.set_workflow_paused(true).unwrap();
    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/background-workers/git-sync/stop".to_string(),
        body: None,
    });
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.body["worker_kind"], "git-sync");
    assert_eq!(stopped.body["status"], "stopped");
    assert!(
        supervisor
            .pause_state()
            .unwrap()
            .disabled_background_workers
            .contains("git-sync")
    );

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/background-workers/git-sync/start".to_string(),
        body: None,
    });
    assert_eq!(started.status, 200, "{}", started.body);
    assert_eq!(started.body["status"], "paused");
    assert!(
        !supervisor
            .pause_state()
            .unwrap()
            .disabled_background_workers
            .contains("git-sync")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_target_app_health_remains_available_while_workflow_is_paused() {
    let temp_root = unique_temp_dir("http-target-status-while-paused");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "target_app_status_command": "printf ok"
        })),
    });
    FileProcessSupervisor::new(&runtime_root)
        .set_workflow_paused(true)
        .unwrap();

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/target-app/status".to_string(),
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["app_url"], "http://127.0.0.1:3000");
    assert_eq!(status.body["has_status_checks"], true);
    assert_eq!(status.body["state"], "unknown");

    let health = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/health".to_string(),
        body: None,
    });
    assert_eq!(health.status, 200);
    assert_eq!(health.body["state"], "running");
    assert_eq!(health.body["last_health_ok"], true);

    remove_temp_dir(&temp_root);
}
