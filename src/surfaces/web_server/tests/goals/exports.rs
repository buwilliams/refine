use super::*;

#[test]
fn web_server_exports_selected_goals_for_jira() {
    let temp_root = unique_temp_dir("http-goals-jira-export");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GOAL1", "Export first delivery"),
        ("GOAL2", "Export second delivery"),
        ("GOAL3", "Leave this Goal out"),
    ] {
        service.create_goal_summary(name, Some(id)).unwrap();
        service
            .append_goal_round_summary(id, "Auditor", &format!("Implement {id}"))
            .unwrap();
        service
            .update_latest_goal_round_implementation_report(
                id,
                &format!("Added Jira evidence for {id}."),
            )
            .unwrap();
    }

    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/export/jira".to_string(),
        body: Some(json!({"selected_ids": ["GOAL2", "GOAL1"]})),
    });

    assert_eq!(response.status, 202);
    assert_eq!(response.body["operation"]["owner"], "goals:jira-export");
    assert_eq!(response.body["operation"]["status"], "running");
    let operation_id = response.body["operation"]["id"].as_str().unwrap();
    let operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        operation_id,
        OperationState::Succeeded,
    );
    assert_eq!(operation.progress["stage"], "complete");
    assert_eq!(operation.progress["completed"], 2);
    assert_eq!(operation.result["export"]["format"], "jira_csv");
    assert_eq!(
        operation.result["export"]["filename"],
        "refine-goals-jira.csv"
    );
    assert_eq!(operation.result["export"]["goal_count"], 2);
    assert_eq!(
        operation.result["export"]["goal_ids"],
        json!(["GOAL1", "GOAL2"])
    );
    let csv = operation.result["export"]["csv"].as_str().unwrap();
    assert!(csv.starts_with("Summary,Description,Work Type,Priority"));
    assert!(csv.contains("Added Jira evidence for GOAL1."));
    assert!(csv.contains("Added Jira evidence for GOAL2."));
    assert!(!csv.contains("Leave this Goal out"));

    let empty = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/export/jira".to_string(),
        body: Some(json!({"selected_ids": []})),
    });
    assert_eq!(empty.status, 202);
    let empty_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        empty.body["operation"]["id"].as_str().unwrap(),
        OperationState::Failed,
    );
    assert_eq!(
        empty_operation.error.unwrap()["message"],
        "Select at least one Goal to export for Jira"
    );

    let (logs, _, _) = FileOperationRegistry::new(&runtime_root)
        .page_logs(operation_id, 20, 0)
        .unwrap();
    assert!(
        logs.iter()
            .any(|entry| entry.message == "Jira CSV export completed")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_delegates_cancel_and_recovers_durable_jira_exports() {
    let temp_root = unique_temp_dir("http-goals-jira-export-lifecycle");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Recoverable export", Some("GOAL1"))
        .unwrap();

    let registry = FileOperationRegistry::new(&runtime_root);
    let request = json!({
        "refine_dir": refine_dir.clone(),
        "target_root": temp_root.clone(),
        "selection": {"selected_ids": ["GOAL1"], "exclude_ids": [], "filter": {}}
    });
    let cancellable = registry
        .register_with_request("goals:jira-export", request.clone())
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let cancellable_process = supervisor
        .launch(operation_helper_process_spec(&cancellable.id))
        .unwrap();
    let cancellable_pid = cancellable_process.pid.unwrap();
    assert!(managed_pid_is_alive(cancellable_pid).unwrap());

    let cancelled = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{}/cancel", cancellable.id),
        body: None,
    });
    assert_eq!(cancelled.status, 200);
    assert_eq!(cancelled.body["operation"]["status"], "cancelled");
    wait_for_managed_pid_exit(cancellable_pid);
    assert!(!managed_pid_is_alive(cancellable_pid).unwrap());
    assert_eq!(
        registry.status(&cancellable.id).unwrap().state,
        OperationState::Cancelled
    );
    let late_completion = registry
        .finish_with_result(
            &cancellable.id,
            OperationState::Succeeded,
            json!({"export": {"csv": "must not become visible"}}),
        )
        .unwrap();
    assert_eq!(late_completion.state, OperationState::Cancelled);
    assert_eq!(
        registry.status(&cancellable.id).unwrap().state,
        OperationState::Cancelled
    );
    let late_failure = registry
        .fail_with_error(
            &cancellable.id,
            json!({
                "code": "late_worker_failure",
                "message": "worker failed after cancellation"
            }),
        )
        .unwrap();
    assert_eq!(late_failure.state, OperationState::Cancelled);
    assert_eq!(late_failure.error.unwrap()["code"], "late_worker_failure");
    assert_eq!(
        registry.status(&cancellable.id).unwrap().state,
        OperationState::Cancelled
    );
    let (cancel_logs, _, _) = registry.page_logs(&cancellable.id, 20, 0).unwrap();
    assert!(
        cancel_logs
            .iter()
            .any(|entry| entry.message == "Operation cancelled")
    );
    assert!(
        cancel_logs
            .iter()
            .any(|entry| entry.message == "Operation failed")
    );

    let interrupted = registry
        .register_with_request("goals:jira-export", request)
        .unwrap();
    registry
        .append_log(
            &interrupted.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "Jira export worker acquired durable ownership".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    let interrupted_process = supervisor
        .launch(operation_helper_process_spec(&interrupted.id))
        .unwrap();
    let interrupted_pid = interrupted_process.pid.unwrap();
    assert!(managed_pid_is_alive(interrupted_pid).unwrap());

    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: temp_root.join("run"),
    });
    let recovery_status = lifecycle.restart(8080).unwrap();
    wait_for_managed_pid_exit(interrupted_pid);
    assert!(!managed_pid_is_alive(interrupted_pid).unwrap());
    assert!(
        recovery_status
            .degraded_integrations
            .contains(&"operation-recovery-interrupted".to_string())
    );
    assert_eq!(
        registry.status(&interrupted.id).unwrap().state,
        OperationState::Interrupted
    );
    assert_eq!(
        registry.status(&interrupted.id).unwrap().error.unwrap()["code"],
        "operation_interrupted"
    );
    let (interruption_logs, _, _) = registry.page_logs(&interrupted.id, 20, 0).unwrap();
    assert!(
        interruption_logs
            .iter()
            .any(|entry| entry.message == "Operation interrupted")
    );
    assert!(
        interruption_logs
            .iter()
            .any(|entry| { entry.message == "Jira export worker acquired durable ownership" })
    );

    let recovered_response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/goals/export/jira/{}/retry", interrupted.id),
        body: Some(json!({})),
    });
    assert_eq!(
        recovered_response.status, 202,
        "{:#}",
        recovered_response.body
    );
    let recovered_id = recovered_response.body["operation"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let recovered_process = supervisor
        .list()
        .unwrap()
        .into_iter()
        .find(|process| {
            process.details.as_deref().is_some_and(|details| {
                serde_json::from_str::<serde_json::Value>(details)
                    .ok()
                    .and_then(|details| details["operation_id"].as_str().map(str::to_string))
                    .as_deref()
                    == Some(recovered_id.as_str())
            })
        })
        .expect("retry should launch an operation-associated managed worker");
    let recovered_pid = recovered_process.pid.unwrap();
    assert!(managed_pid_is_alive(recovered_pid).unwrap());
    let recovered_operation =
        wait_for_operation_status(&registry, &recovered_id, OperationState::Succeeded);
    assert_eq!(recovered_operation.request["recovery_of"], interrupted.id);
    assert_eq!(recovered_operation.result["export"]["goal_count"], 1);
    let (recovered_logs, _, _) = registry.page_logs(&recovered_id, 20, 0).unwrap();
    assert!(
        recovered_logs
            .iter()
            .any(|entry| entry.message == "Jira CSV export completed")
    );
    let (original_logs, _, _) = registry.page_logs(&interrupted.id, 20, 0).unwrap();
    assert_eq!(
        original_logs
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        interruption_logs
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>()
    );
    wait_for_managed_pid_exit(recovered_pid);

    let failure_runtime_root = temp_root.join("run/cancel-failure");
    let failure_registry = FileOperationRegistry::new(&failure_runtime_root);
    let termination_failure = failure_registry.register("goals:jira-export").unwrap();
    fs::create_dir_all(&failure_runtime_root).unwrap();
    fs::write(failure_runtime_root.join("processes"), b"not a directory").unwrap();
    server.runtime_root = Some(failure_runtime_root);
    let failed_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{}/cancel", termination_failure.id),
        body: None,
    });
    assert_eq!(failed_cancel.status, 500);
    let termination_failure = failure_registry.status(&termination_failure.id).unwrap();
    assert_eq!(termination_failure.state, OperationState::Cancelled);
    assert_eq!(
        termination_failure.error.unwrap()["code"],
        "operation_process_termination_failed"
    );
    let (failure_logs, _, _) = failure_registry
        .page_logs(&termination_failure.id, 20, 0)
        .unwrap();
    assert!(
        failure_logs
            .iter()
            .any(|entry| entry.message == "Operation failed")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn concurrent_jira_retry_posts_share_one_durable_replacement_and_worker_after_restart() {
    let temp_root = unique_temp_dir("http-goals-jira-export-concurrent-retry");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    FileWorkItemService::new(&refine_dir)
        .create_goal_summary("Concurrent recoverable export", Some("GOAL1"))
        .unwrap();

    let registry = FileOperationRegistry::new(&runtime_root);
    let source = registry
        .register_with_request(
            "goals:jira-export",
            json!({
                "refine_dir": refine_dir,
                "target_root": temp_root,
                "selection": {"selected_ids": ["GOAL1"], "exclude_ids": [], "filter": {}}
            }),
        )
        .unwrap();
    registry
        .append_log(
            &source.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "Source log retained across concurrent retry".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    registry.interrupt_active().unwrap();
    let (source_logs_before, _, _) = registry.page_logs(&source.id, 20, 0).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let barrier = Arc::new(Barrier::new(3));
    let callers = (0..2)
        .map(|_| {
            let server = server.clone();
            let barrier = Arc::clone(&barrier);
            let path = format!("/api/goals/export/jira/{}/retry", source.id);
            thread::spawn(move || {
                barrier.wait();
                server.handle(ApiRequest {
                    method: "POST".to_string(),
                    path,
                    body: Some(json!({})),
                })
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let responses = callers
        .into_iter()
        .map(|caller| caller.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        responses.iter().all(|response| response.status == 202),
        "{responses:#?}"
    );
    let replacement_ids = responses
        .iter()
        .map(|response| {
            response.body["operation"]["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(replacement_ids[0], replacement_ids[1]);
    let replacement_id = &replacement_ids[0];
    let retry_identity = format!("goals:jira-export:retry:{}", source.id);

    let replacements = registry
        .recover()
        .unwrap()
        .into_iter()
        .filter(|operation| operation.request["recovery_of"] == source.id)
        .collect::<Vec<_>>();
    assert_eq!(replacements.len(), 1);
    assert_eq!(replacements[0].id, *replacement_id);
    assert_eq!(replacements[0].request["retry_identity"], retry_identity);

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let replacement_processes = supervisor
        .list()
        .unwrap()
        .into_iter()
        .filter(|process| {
            process.details.as_deref().is_some_and(|details| {
                serde_json::from_str::<serde_json::Value>(details)
                    .ok()
                    .and_then(|details| details["operation_id"].as_str().map(str::to_string))
                    .as_deref()
                    == Some(replacement_id.as_str())
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replacement_processes.len(),
        1,
        "concurrent retries must launch exactly one managed Jira worker"
    );
    let replacement_pid = replacement_processes[0].pid.unwrap();
    assert!(managed_pid_is_alive(replacement_pid).unwrap());

    let replacement =
        wait_for_operation_status(&registry, replacement_id, OperationState::Succeeded);
    assert_eq!(replacement.result["export"]["goal_count"], 1);
    assert_eq!(
        registry
            .recover()
            .unwrap()
            .iter()
            .filter(|operation| operation.request["recovery_of"] == source.id)
            .filter(|operation| matches!(operation.state, OperationState::Succeeded))
            .count(),
        1
    );
    let (source_logs_after, _, _) = registry.page_logs(&source.id, 20, 0).unwrap();
    assert_eq!(source_logs_after, source_logs_before);
    assert!(
        source_logs_after
            .iter()
            .any(|entry| entry.message == "Source log retained across concurrent retry")
    );
    wait_for_managed_pid_exit(replacement_pid);
    assert!(!managed_pid_is_alive(replacement_pid).unwrap());

    FileDaemonLifecycleService::new(RuntimeRoot {
        root: temp_root.join("run"),
    })
    .restart(8080)
    .unwrap();
    let processes_before_replay = supervisor.list().unwrap();
    let mut restarted_server = server_with_projection();
    restarted_server.target_root = Some(temp_root.clone());
    restarted_server.runtime_root = Some(runtime_root.clone());
    let replay = restarted_server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/goals/export/jira/{}/retry", source.id),
        body: Some(json!({})),
    });
    assert_eq!(replay.status, 202, "{:#}", replay.body);
    assert_eq!(replay.body["operation"]["id"], *replacement_id);
    assert_eq!(supervisor.list().unwrap(), processes_before_replay);
    assert_eq!(
        registry.status(replacement_id).unwrap().state,
        OperationState::Succeeded
    );
    assert_eq!(
        registry.page_logs(&source.id, 20, 0).unwrap().0,
        source_logs_before
    );

    remove_temp_dir(&temp_root);
}
