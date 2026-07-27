use super::*;

#[test]
fn resistant_agent_retains_process_evidence_and_goal_state() {
    let temp_root = unique_temp_dir("process-control-resistant");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal(&refine_dir, "GOAL-RESIST");
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(
        &supervisor,
        "GOAL-RESIST",
        Some(("sh", vec!["-c", "trap '' TERM; while :; do sleep 1; done"])),
    );

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_agent_exit_timeout(Duration::from_millis(100))
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(matches!(error, RefineError::Degraded(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("identity evidence were retained")
    );
    assert!(error.to_string().contains("remains non-cancelled"));
    assert!(supervisor.inspect(&process.id).is_ok());
    assert!(
        runtime_root
            .join("agents/process-identities")
            .join(format!("{}.json", process.id))
            .exists()
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-RESIST")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    supervisor.request_termination(&process.id, "kill").unwrap();
    wait_for_exit(process.pid.unwrap());
    let _ = supervisor.recover();
    remove_temp_dir(&temp_root);
}

#[test]
fn ownership_change_after_confirmed_exit_retains_truthful_partial_outcome() {
    let temp_root = unique_temp_dir("process-control-post-exit-ownership");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-POST-EXIT", 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(
        &supervisor,
        "GOAL-POST-EXIT",
        "claim-current",
        "exec-current",
        0,
    );
    let pid = process.pid.unwrap();
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-current",
            "goal_id": "GOAL-POST-EXIT",
            "execution_id": "exec-current",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );

    let hook_runtime = runtime_root.clone();
    let hook_target = temp_root.clone();
    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_post_exit_hook(move || {
            WorkflowEngine::with_target_root(&hook_runtime, &hook_target)
                .retry("exec-current")
                .unwrap();
        })
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    let message = error.to_string();
    assert!(message.contains("confirmed_exit=true"), "{message}");
    assert!(
        message.contains("registry_cleanup_completed=true"),
        "{message}"
    );
    assert!(
        message.contains("identity_cleanup_completed=true"),
        "{message}"
    );
    assert!(message.contains("goal_cancelled=false"), "{message}");
    assert!(
        message.contains("claim identity or execution changed"),
        "{message}"
    );
    assert!(message.contains("supported recovery"), "{message}");
    assert!(
        !message.contains("termination was not requested"),
        "{message}"
    );
    assert!(!managed_pid_is_alive(pid).unwrap());
    assert!(supervisor.inspect(&process.id).is_err());
    assert!(
        !runtime_root
            .join("agents/process-identities")
            .join(format!("{}.json", process.id))
            .exists()
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-POST-EXIT")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    let receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["state"], "partial_failure");
    assert_eq!(receipt["confirmed_exit"], true);
    assert_eq!(receipt["registry_cleanup_completed"], true);
    assert_eq!(receipt["identity_cleanup_completed"], true);
    assert_eq!(receipt["goal_cancelled"], false);
    assert!(
        receipt["cause"]
            .as_str()
            .unwrap()
            .contains("claim identity or execution changed")
    );
    assert!(
        receipt["recovery"]
            .as_str()
            .unwrap()
            .contains("shared Process capability")
    );

    remove_temp_dir(&temp_root);
}
