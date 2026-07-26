use super::*;

#[test]
fn pid_identity_mismatch_never_signals_or_cancels() {
    let temp_root = unique_temp_dir("process-control-identity");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal(&refine_dir, "GOAL-IDENTITY");
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(&supervisor, "GOAL-IDENTITY", None);
    let identity_path = runtime_root
        .join("agents/process-identities")
        .join(format!("{}.json", process.id));
    let mut identity: Value = serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
    identity["os_identity"] = json!("linux:different-boot:different-start");
    fs::write(
        &identity_path,
        serde_json::to_vec_pretty(&identity).unwrap(),
    )
    .unwrap();

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(error.to_string().contains("PID identity mismatch"));
    assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert!(supervisor.inspect(&process.id).is_ok());
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-IDENTITY")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    Command::new("kill")
        .args(["-KILL", &process.pid.unwrap().to_string()])
        .status()
        .unwrap();
    wait_for_exit(process.pid.unwrap());
    remove_temp_dir(&temp_root);
}

#[test]
fn legacy_missing_identity_never_adopts_current_pid_or_cancels() {
    let temp_root = unique_temp_dir("process-control-legacy-identity");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal(&refine_dir, "GOAL-LEGACY-IDENTITY");
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(&supervisor, "GOAL-LEGACY-IDENTITY", None);
    let identity_path = runtime_root
        .join("agents/process-identities")
        .join(format!("{}.json", process.id));
    fs::remove_file(&identity_path).unwrap();
    let registration_error = supervisor.register(process.clone()).unwrap_err();
    assert!(
        registration_error
            .to_string()
            .contains("no registration-time PID identity evidence")
    );

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("no registration-time PID identity evidence")
    );
    assert!(
        error
            .to_string()
            .contains("recorded PID may have been reused")
    );
    assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert!(supervisor.inspect(&process.id).is_ok());
    assert!(
        !identity_path.exists(),
        "stop-time control must not invent identity evidence"
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-LEGACY-IDENTITY")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    force_kill(process.pid.unwrap());
    wait_for_exit(process.pid.unwrap());
    remove_temp_dir(&temp_root);
}

#[test]
fn identity_cleanup_failure_retains_registry_result_and_confirmed_exit() {
    let temp_root = unique_temp_dir("process-control-identity-cleanup-failure");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-IDENTITY-CLEANUP", 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = register_workflow_agent(
        &supervisor,
        "GOAL-IDENTITY-CLEANUP",
        "claim-current",
        "exec-current",
        0,
    );
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-current",
            "goal_id": "GOAL-IDENTITY-CLEANUP",
            "execution_id": "exec-current",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_cleanup_failure(ProcessCleanupStage::Identity)
        .stop(&process.id, "terminate")
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("confirmed_exit=true"), "{message}");
    assert!(
        message.contains("registry_cleanup_completed=true"),
        "{message}"
    );
    assert!(
        message.contains("identity_cleanup_completed=false"),
        "{message}"
    );
    assert!(message.contains("goal_cancelled=false"), "{message}");
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert!(supervisor.inspect(&process.id).is_err());
    assert!(
        runtime_root
            .join("agents/process-identities")
            .join(format!("{}.json", process.id))
            .exists()
    );
    assert_partial_cleanup_receipt(
        &runtime_root,
        &process.id,
        true,
        false,
        "injected identity cleanup failure",
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-IDENTITY-CLEANUP")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Running
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn stale_execution_and_round_never_stop_or_cancel_newer_goal_work() {
    let temp_root = unique_temp_dir("process-control-stale-execution");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-STALE", 2);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, "GOAL-STALE", "claim-old", "exec-old", 0);
    write_workflow_state(
        &runtime_root,
        json!([
            {
                "claim_id": "claim-old",
                "goal_id": "GOAL-STALE",
                "execution_id": "exec-old",
                "state": "failed",
                "created_at": "2026-07-23T00:00:00Z",
                "updated_at": "2026-07-23T00:01:00Z"
            },
            {
                "claim_id": "claim-new",
                "goal_id": "GOAL-STALE",
                "execution_id": "exec-new",
                "state": "running",
                "created_at": "2026-07-23T00:02:00Z",
                "updated_at": "2026-07-23T00:02:00Z"
            }
        ]),
    );

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(error.to_string().contains("stale workflow ownership"));
    assert!(error.to_string().contains("newer workflow claim"));
    assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert!(supervisor.inspect(&process.id).is_ok());
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-STALE")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    force_kill(process.pid.unwrap());
    wait_for_exit(process.pid.unwrap());
    remove_temp_dir(&temp_root);
}

#[test]
fn current_execution_with_stale_round_never_stops_or_cancels() {
    let temp_root = unique_temp_dir("process-control-stale-round");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-STALE-ROUND", 2);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(
        &supervisor,
        "GOAL-STALE-ROUND",
        "claim-current",
        "exec-current",
        0,
    );
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-current",
            "goal_id": "GOAL-STALE-ROUND",
            "execution_id": "exec-current",
            "state": "running",
            "created_at": "2026-07-23T00:02:00Z",
            "updated_at": "2026-07-23T00:02:00Z"
        }]),
    );

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("process round 1 is not the current Goal round 2")
    );
    assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert!(supervisor.inspect(&process.id).is_ok());
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-STALE-ROUND")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    force_kill(process.pid.unwrap());
    wait_for_exit(process.pid.unwrap());
    remove_temp_dir(&temp_root);
}
