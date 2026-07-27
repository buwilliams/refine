use super::*;

#[test]
fn confirmed_agent_exit_precedes_linked_goal_cancellation() {
    let temp_root = unique_temp_dir("process-control-confirmed");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal(&refine_dir, "GOAL-CONFIRMED");
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(&supervisor, "GOAL-CONFIRMED", None);
    let pid = process.pid.unwrap();

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(result["stopped"], true);
    assert_eq!(result["termination"]["confirmed_exit"], true);
    assert_eq!(result["termination"]["registry_retained_until_exit"], true);
    assert!(!managed_pid_is_alive(pid).unwrap());
    assert!(supervisor.inspect(&process.id).is_err());
    assert_eq!(result["goal"]["status"], "cancelled");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-CONFIRMED")
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn current_workflow_execution_can_stop_and_cancel_its_goal() {
    let temp_root = unique_temp_dir("process-control-current-execution");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-CURRENT", 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = register_workflow_agent(
        &supervisor,
        "GOAL-CURRENT",
        "claim-current",
        "exec-current",
        0,
    );
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-current",
            "goal_id": "GOAL-CURRENT",
            "execution_id": "exec-current",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(result["termination"]["confirmed_exit"], true);
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert_eq!(result["goal"]["status"], "cancelled");
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    assert_eq!(state.claims[0].state, WorkflowClaimState::Cancelled);
    assert!(
        crate::workflow::capacity::AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    remove_temp_dir(&temp_root);
}

#[test]
fn target_bound_cancellation_before_worker_registration_fails_closed() {
    let temp_root = unique_temp_dir("process-control-before-registration");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-REGISTERING", 1);
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-registering",
            "goal_id": "GOAL-REGISTERING",
            "execution_id": "exec-registering",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );
    reserve_workflow_capacity(&runtime_root, "claim-registering");

    let control = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir);
    let error = control
        .cancel_workflow_execution("exec-registering")
        .unwrap_err();
    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("empty lookup is not confirmed process exit"),
        "{error}"
    );
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Running
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-REGISTERING")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert_eq!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .len(),
        1
    );

    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(
        &supervisor,
        "GOAL-REGISTERING",
        "claim-registering",
        "exec-registering",
        0,
    );
    assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
    let stopped = control.stop(&process.id, "terminate").unwrap();
    assert_eq!(stopped["goal"]["status"], "cancelled");
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Cancelled
    );
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn successful_cancellation_preserves_complete_non_default_workflow_policy() {
    let temp_root = unique_temp_dir("process-control-policy-success");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let policy = non_default_workflow_policy();
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-POLICY-SUCCESS", 1);
    write_workflow_state_with_policy(
        &runtime_root,
        json!([{
            "claim_id": "claim-policy-success",
            "goal_id": "GOAL-POLICY-SUCCESS",
            "node_id": policy.active_node_id,
            "provider": policy.provider,
            "target_app_id": policy.target_app_id,
            "execution_id": "exec-policy-success",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
        &policy,
    );
    reserve_workflow_capacity_with_policy(&runtime_root, "claim-policy-success", &policy);
    let policy_bytes = serde_json::to_vec(&policy).unwrap();

    let goal = preflight_goal_state(&refine_dir, "GOAL-POLICY-SUCCESS").unwrap();
    let cancelled = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .settle_goal_cancellation(
            &refine_dir,
            "GOAL-POLICY-SUCCESS",
            &goal,
            &[WorkflowGoalOwnership {
                process_id: "confirmed-policy-process".to_string(),
                claim_id: "claim-policy-success".to_string(),
                execution_id: Some("exec-policy-success".to_string()),
                round_idx: Some(0),
            }],
        )
        .unwrap();

    assert_eq!(cancelled.goal.status, GoalStatus::Cancelled);
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    assert_eq!(serde_json::to_vec(&state.policy).unwrap(), policy_bytes);
    assert_eq!(state.claims[0].state, WorkflowClaimState::Cancelled);
    assert_eq!(state.version, 1);
    assert_eq!(state.claims[0].decision_version, 1);
    assert_eq!(state.claims[0].node_id, "node-policy");
    assert_eq!(state.claims[0].provider, "provider-policy");
    assert_eq!(state.claims[0].target_app_id, "/srv/non-default-target");

    remove_temp_dir(&temp_root);
}

#[test]
fn final_ownership_and_goal_fence_are_atomic_with_cancellation() {
    let temp_root = unique_temp_dir("process-control-atomic-settlement");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-ATOMIC", 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(
        &supervisor,
        "GOAL-ATOMIC",
        "claim-current",
        "exec-current",
        0,
    );
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-current",
            "goal_id": "GOAL-ATOMIC",
            "execution_id": "exec-current",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );

    let (at_fence_tx, at_fence_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let hook_release = Arc::clone(&release_rx);
    let service = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_settlement_hook(move || {
            at_fence_tx.send(()).unwrap();
            hook_release.lock().unwrap().recv().unwrap();
        });
    let stopped_process_id = process.id.clone();
    let stop_thread = thread::spawn(move || service.stop(&stopped_process_id, "terminate"));

    at_fence_rx.recv().unwrap();
    let (attempted_tx, attempted_rx) = mpsc::channel();
    let round_refine_dir = refine_dir.clone();
    let round_attempted = attempted_tx.clone();
    let round_thread = thread::spawn(move || {
        round_attempted.send("round").unwrap();
        FileWorkItemService::new(round_refine_dir).append_goal_round_summary(
            "GOAL-ATOMIC",
            "Concurrent owner",
            "Start a newer round",
        )
    });
    let retry_runtime = runtime_root.clone();
    let retry_target = temp_root.clone();
    let retry_thread = thread::spawn(move || {
        attempted_tx.send("retry").unwrap();
        WorkflowEngine::with_target_root(retry_runtime, retry_target).retry("exec-current")
    });
    let mut attempted = vec![attempted_rx.recv().unwrap(), attempted_rx.recv().unwrap()];
    attempted.sort_unstable();
    assert_eq!(attempted, vec!["retry", "round"]);

    release_tx.send(()).unwrap();
    let stop_result = stop_thread.join().unwrap().unwrap();
    let round_error = round_thread.join().unwrap().unwrap_err();
    let retry_error = retry_thread.join().unwrap().unwrap_err();

    assert_eq!(stop_result["termination"]["confirmed_exit"], true);
    assert_eq!(stop_result["goal"]["status"], "cancelled");
    assert!(
        round_error.to_string().contains("not allowed"),
        "{round_error}"
    );
    assert!(
        retry_error
            .to_string()
            .contains("workflow execution cannot be retried"),
        "{retry_error}"
    );
    let goal = FileWorkItemService::new(&refine_dir)
        .show_goal_summary("GOAL-ATOMIC")
        .unwrap();
    assert_eq!(goal.goal.status, GoalStatus::Cancelled);
    assert_eq!(goal.goal.round_count, 1);
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.claim_id == "claim-current")
        .unwrap();
    assert_eq!(claim.execution_id.as_deref(), Some("exec-current"));
    assert_eq!(claim.state, WorkflowClaimState::Cancelled);
    assert!(
        crate::workflow::capacity::AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );

    remove_temp_dir(&temp_root);
}
