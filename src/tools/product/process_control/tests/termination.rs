use super::*;

#[test]
fn confirmed_agent_exit_precedes_linked_goal_requeue() {
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
    assert_eq!(result["goal"]["status"], "todo");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-CONFIRMED")
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn current_workflow_execution_can_stop_and_requeue_its_goal() {
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
    assert_eq!(result["goal"]["status"], "todo");
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
fn interactive_stop_fails_a_planned_attempt_instead_of_requeueing_its_round() {
    let temp_root = unique_temp_dir("process-control-planned-stop");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-PLANNED-STOP";
    let claim_id = "claim-planned-stop";
    let execution_id = "exec-planned-stop";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);
    let original_plan = seed_in_progress_implementation_plan(
        &refine_dir,
        goal_id,
        claim_id,
        execution_id,
        &process.id,
    );

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(result["termination"]["confirmed_exit"], true);
    assert_eq!(result["goal"]["status"], "failed");
    assert_eq!(result["goal_requeued"], false);
    assert_eq!(result["goal_disposition"], "fail_attempt");
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    assert_eq!(state.claims[0].state, WorkflowClaimState::Cancelled);
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    let service = FileWorkItemService::new(&refine_dir);
    let detail = service.show_goal_detail(goal_id).unwrap();
    let failed_plan = &detail["rounds"][0]["implementation_plan"];
    assert_eq!(failed_plan["state"], "failed");
    assert_eq!(failed_plan["failure"]["category"], "interrupted");
    assert_eq!(failed_plan["failure"]["process_id"], process.id);
    assert_eq!(failed_plan["binding"]["claim_id"], claim_id);
    assert_eq!(failed_plan["binding"]["execution_id"], execution_id);
    assert_eq!(failed_plan["proposal"], json!(original_plan.proposal));

    service
        .append_goal_round_summary(goal_id, "Recovery", "Start a fresh planned attempt")
        .unwrap();
    service
        .transition_goal_status(goal_id, GoalStatus::Todo)
        .unwrap();
    let recovered = service.show_goal_detail(goal_id).unwrap();
    assert_eq!(recovered["rounds"].as_array().unwrap().len(), 2);
    assert!(recovered["rounds"][1]["implementation_plan"].is_null());

    remove_temp_dir(&temp_root);
}

#[test]
fn interrupted_planned_stop_replays_the_failed_attempt_disposition() {
    let temp_root = unique_temp_dir("process-control-planned-stop-replay");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-PLANNED-STOP-REPLAY";
    let claim_id = "claim-planned-stop-replay";
    let execution_id = "exec-planned-stop-replay";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);
    seed_in_progress_implementation_plan(&refine_dir, goal_id, claim_id, execution_id, &process.id);

    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_settlement_interruption(CancellationSettlementFailureStage::CapacityRelease)
            .stop(&process.id, "terminate")
            .unwrap();
    }));
    assert!(interrupted.is_err());

    let replayed = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(replayed["goal"]["status"], "failed");
    assert_eq!(replayed["goal_disposition"], "fail_attempt");
    assert_eq!(replayed["goal_requeued"], false);
    let journal: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(journal["schema_version"], 7);
    assert_eq!(journal["state"], "committed");
    assert_eq!(journal["goal_disposition"], "fail_attempt");
    assert_eq!(journal["goal_requeued"], false);
    let detail = FileWorkItemService::new(&refine_dir)
        .show_goal_detail(goal_id)
        .unwrap();
    assert_eq!(
        detail["rounds"][0]["implementation_plan"]["state"],
        "failed"
    );
    assert_eq!(
        detail["rounds"][0]["implementation_plan"]["failure"]["category"],
        "interrupted"
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn explicit_cancellation_fails_active_planning_evidence_and_remains_terminal() {
    let temp_root = unique_temp_dir("process-control-planned-cancel");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-PLANNED-CANCEL";
    let claim_id = "claim-planned-cancel";
    let execution_id = "exec-planned-cancel";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);
    seed_in_progress_implementation_plan(&refine_dir, goal_id, claim_id, execution_id, &process.id);

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(execution_id)
        .unwrap();

    assert_eq!(result["cancelled"], true);
    assert_eq!(result["goal"]["status"], "cancelled");
    assert_eq!(result["goal_disposition"], "cancel");
    let detail = FileWorkItemService::new(&refine_dir)
        .show_goal_detail(goal_id)
        .unwrap();
    assert_eq!(
        detail["rounds"][0]["implementation_plan"]["state"],
        "failed"
    );
    assert_eq!(
        detail["rounds"][0]["implementation_plan"]["failure"]["category"],
        "cancelled"
    );
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Cancelled
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
    assert_eq!(stopped["goal"]["status"], "todo");
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
            TerminationIntent::ExplicitCancellation,
            &[],
        )
        .unwrap();

    assert_eq!(cancelled.goal.goal.status, GoalStatus::Cancelled);
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
fn final_ownership_and_goal_fence_are_atomic_with_requeue() {
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
    thread::sleep(Duration::from_millis(20));
    assert!(!round_thread.is_finished());
    assert!(!retry_thread.is_finished());

    release_tx.send(()).unwrap();
    let stop_result = stop_thread.join().unwrap().unwrap();
    let round = round_thread.join().unwrap().unwrap();
    let retried_execution_id = retry_thread.join().unwrap().unwrap();

    assert_eq!(stop_result["termination"]["confirmed_exit"], true);
    assert_eq!(stop_result["goal"]["status"], "todo");
    assert_eq!(round.goal.status, GoalStatus::Todo);
    assert_ne!(retried_execution_id, "exec-current");
    let goal = FileWorkItemService::new(&refine_dir)
        .show_goal_summary("GOAL-ATOMIC")
        .unwrap();
    assert_eq!(goal.goal.status, GoalStatus::Todo);
    assert_eq!(goal.goal.round_count, 2);
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.claim_id == "claim-current")
        .unwrap();
    assert_eq!(
        claim.execution_id.as_deref(),
        Some(retried_execution_id.as_str())
    );
    assert_eq!(claim.state, WorkflowClaimState::Running);
    assert!(
        crate::workflow::capacity::AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .iter()
            .any(|lease| lease.owner_id == "workflow:claim-current")
    );

    remove_temp_dir(&temp_root);
}
