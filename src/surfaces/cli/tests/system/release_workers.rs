use super::*;

#[test]
fn system_start_recovers_jira_export_and_public_retry_launches_supervised_worker() {
    let temp_root = unique_temp_dir("cli-system-start-jira-recovery");
    let runtime_root = temp_root.join("run");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    FileWorkItemService::new(&refine_dir)
        .create_goal_summary("Restart-safe Jira export", Some("GOAL1"))
        .unwrap();

    let port_probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    let registry = FileOperationRegistry::new(&port_runtime_root);
    let interrupted = registry
        .register_with_request(
            "goals:jira-export",
            serde_json::json!({
                "refine_dir": refine_dir,
                "target_root": target_root,
                "selection": {"selected_ids": ["GOAL1"], "exclude_ids": [], "filter": {}}
            }),
        )
        .unwrap();
    registry
        .append_log(
            &interrupted.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "Durable log before production restart".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&port_runtime_root);
    let worker = supervisor
        .launch(cli_operation_helper_process_spec(&interrupted.id))
        .unwrap();
    let worker_pid = worker.pid.unwrap();
    assert!(managed_pid_is_alive(worker_pid).unwrap());

    // Model an abrupt daemon exit: the durable registration and its worker are both still live
    // when the production startup path begins recovery. Deliberately do not call lifecycle.stop.
    assert!(managed_pid_is_alive(worker_pid).unwrap());
    assert_eq!(
        registry.status(&interrupted.id).unwrap().state,
        OperationState::Running
    );

    let start_runtime_root = runtime_root.clone();
    let server = thread::spawn(move || {
        run_system_start(
            port,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
            None,
            start_runtime_root,
            true,
            true,
        )
    });
    let interrupted_after_start =
        wait_for_cli_operation_state(&registry, &interrupted.id, OperationState::Interrupted);
    wait_for_cli_managed_pid_exit(worker_pid);
    assert!(!managed_pid_is_alive(worker_pid).unwrap());
    assert_eq!(
        interrupted_after_start.error.unwrap()["code"],
        "operation_interrupted"
    );
    let registry_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while supervisor
        .list()
        .unwrap()
        .iter()
        .any(|process| process.id == worker.id)
        && std::time::Instant::now() < registry_deadline
    {
        thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        supervisor
            .list()
            .unwrap()
            .iter()
            .all(|process| process.id != worker.id),
        "startup recovery must remove the original worker before exposing retry"
    );

    let mut stream = connect_to_cli_test_daemon(port);
    let retry_path = format!("/api/goals/export/jira/{}/retry", interrupted.id);
    let request = format!(
        "POST {retry_path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    server.join().unwrap().unwrap();
    assert!(response.starts_with("HTTP/1.1 202"), "{response}");
    let response_body = response.split("\r\n\r\n").nth(1).unwrap();
    let response_body: serde_json::Value = serde_json::from_str(response_body).unwrap();
    let recovered_id = response_body["operation"]["id"].as_str().unwrap();
    let recovered_processes = supervisor
        .list()
        .unwrap()
        .into_iter()
        .filter(|process| {
            process.details.as_deref().is_some_and(|details| {
                serde_json::from_str::<serde_json::Value>(details)
                    .ok()
                    .and_then(|details| details["operation_id"].as_str().map(str::to_string))
                    .as_deref()
                    == Some(recovered_id)
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recovered_processes.len(),
        1,
        "public retry must launch exactly one replacement worker"
    );
    let recovered_process = &recovered_processes[0];
    assert!(managed_pid_is_alive(recovered_process.pid.unwrap()).unwrap());
    let recovered =
        wait_for_cli_operation_state(&registry, recovered_id, OperationState::Succeeded);
    assert_eq!(recovered.request["recovery_of"], interrupted.id);
    assert_eq!(recovered.result["export"]["goal_count"], 1);
    wait_for_cli_operation_log(&registry, &interrupted.id, "Operation interrupted");
    let (original_logs, _, _) = registry.page_logs(&interrupted.id, 20, 0).unwrap();
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Durable log before production restart")
    );
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Operation interrupted")
    );
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Recovery terminating managed process")
    );
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Recovery confirmed managed process exit")
    );
    wait_for_cli_operation_log(&registry, recovered_id, "Jira CSV export completed");
    let (recovered_logs, _, _) = registry.page_logs(recovered_id, 20, 0).unwrap();
    assert!(
        recovered_logs
            .iter()
            .any(|entry| entry.message == "Jira CSV export completed")
    );
    wait_for_cli_managed_pid_exit(recovered_process.pid.unwrap());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_start_refuses_jira_retry_when_orphan_exit_cannot_be_confirmed() {
    let temp_root = unique_temp_dir("cli-system-start-jira-recovery-attention");
    let runtime_root = temp_root.join("run");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();

    let port_probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    let registry = FileOperationRegistry::new(&port_runtime_root);
    let operation = registry
        .register_with_request(
            "goals:jira-export",
            serde_json::json!({
                "refine_dir": refine_dir,
                "target_root": target_root,
                "selection": {"selected_ids": [], "exclude_ids": [], "filter": {}}
            }),
        )
        .unwrap();
    registry
        .append_log(
            &operation.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "Evidence retained before failed recovery".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&port_runtime_root);
    let ready_path = temp_root.join("stubborn-worker-ready");
    let worker = supervisor
        .launch(cli_stubborn_operation_helper_process_spec(
            &operation.id,
            &ready_path,
        ))
        .unwrap();
    let worker_pid = worker.pid.unwrap();
    let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !ready_path.exists() && std::time::Instant::now() < ready_deadline {
        thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        ready_path.exists(),
        "stubborn recovery helper did not start"
    );
    assert!(managed_pid_is_alive(worker_pid).unwrap());

    let start_runtime_root = runtime_root.clone();
    let server = thread::spawn(move || {
        run_system_start(
            port,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
            None,
            start_runtime_root,
            true,
            true,
        )
    });
    let failed = wait_for_cli_operation_state(&registry, &operation.id, OperationState::Failed);
    assert_eq!(
        failed.error.as_ref().unwrap()["code"],
        "operation_recovery_process_termination_failed"
    );
    assert_eq!(failed.error.as_ref().unwrap()["retryable"], false);
    assert_eq!(
        failed.request["target_root"],
        target_root.display().to_string()
    );
    assert!(managed_pid_is_alive(worker_pid).unwrap());

    let mut stream = connect_to_cli_test_daemon(port);
    let retry_path = format!("/api/goals/export/jira/{}/retry", operation.id);
    let request = format!(
        "POST {retry_path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    server.join().unwrap().unwrap();
    assert!(response.starts_with("HTTP/1.1 409"), "{response}");
    assert_eq!(
        registry
            .recover()
            .unwrap()
            .iter()
            .filter(|candidate| {
                candidate
                    .request
                    .get("recovery_of")
                    .and_then(serde_json::Value::as_str)
                    == Some(operation.id.as_str())
            })
            .count(),
        0,
        "a recovery-attention operation must not launch a replacement worker"
    );
    let (logs, _, _) = registry.page_logs(&operation.id, 20, 0).unwrap();
    assert!(
        logs.iter()
            .any(|entry| entry.message == "Evidence retained before failed recovery")
    );
    assert!(
        logs.iter()
            .any(|entry| entry.message == "Recovery could not confirm managed process exit")
    );

    supervisor.request_termination(&worker.id, "kill").unwrap();
    wait_for_cli_managed_pid_exit(worker_pid);
    assert!(!managed_pid_is_alive(worker_pid).unwrap());
    fs::remove_dir_all(temp_root).unwrap();
}
