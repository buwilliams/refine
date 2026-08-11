use super::*;

#[test]
fn interactive_stop_replay_preserves_authoritative_requeue_intent() {
    let temp_root = unique_temp_dir("process-control-stop-intent-replay");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-STOP-REPLAY";
    let claim_id = "claim-stop-replay";
    let execution_id = "exec-stop-replay";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);

    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_settlement_interruption(CancellationSettlementFailureStage::ClaimPersistence)
            .stop(&process.id, "terminate")
            .unwrap();
    }));
    assert!(interrupted.is_err());
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());

    let replayed = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(replayed["stopped"], true);
    assert_eq!(replayed["termination_intent"], "interactive_stop");
    assert_eq!(replayed["replayed_settlement"], true);
    assert_eq!(replayed["goal"]["status"], "todo");
    assert!(replayed.get("cancelled").is_none());
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );
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
    let receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["termination_intent"], "interactive_stop");
    assert_eq!(receipt["goal_cancelled"], false);
    assert_eq!(receipt["goal_requeued"], true);

    remove_temp_dir(&temp_root);
}

#[test]
fn interactive_stop_of_cancelled_live_no_claim_goal_preserves_terminal_state() {
    let temp_root = unique_temp_dir("process-control-cancelled-no-claim-stop");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-CANCELLED-NO-CLAIM";
    create_in_progress_goal(&refine_dir, goal_id);
    FileWorkItemService::new(&refine_dir)
        .cancel_goal_summary(goal_id)
        .unwrap();
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(&supervisor, goal_id, None);

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(result["stopped"], true);
    assert_eq!(result["requested_termination_intent"], "interactive_stop");
    assert_eq!(result["termination_intent"], "explicit_cancellation");
    assert_eq!(result["intent_superseded"], true);
    assert_eq!(result["cancelled"], true);
    assert_eq!(result["goal"]["status"], "cancelled");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    let receipt = process_outcome_receipt(&runtime_root, &process.id);
    assert_eq!(receipt["requested_termination_intent"], "interactive_stop");
    assert_eq!(receipt["termination_intent"], "explicit_cancellation");
    assert_eq!(receipt["goal_cancelled"], true);
    assert_eq!(receipt["goal_requeued"], false);

    remove_temp_dir(&temp_root);
}

#[test]
fn cancellation_between_stop_exit_and_no_claim_settlement_wins() {
    let temp_root = unique_temp_dir("process-control-no-claim-cancel-race");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-NO-CLAIM-CANCEL-RACE";
    create_in_progress_goal(&refine_dir, goal_id);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(&supervisor, goal_id, None);
    let hook_refine_dir = refine_dir.clone();

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_post_exit_hook(move || {
            FileWorkItemService::new(&hook_refine_dir)
                .cancel_goal_summary(goal_id)
                .unwrap();
        })
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(result["stopped"], true);
    assert_eq!(result["termination_intent"], "explicit_cancellation");
    assert_eq!(result["goal"]["status"], "cancelled");
    assert_eq!(result["intent_superseded"], true);
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn cancellation_before_and_during_active_claim_stop_is_monotonic() {
    for (suffix, cancel_before_stop) in [("before", true), ("during", false)] {
        let temp_root = unique_temp_dir(&format!("process-control-claim-cancel-{suffix}"));
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        let goal_id = format!("GOAL-CLAIM-CANCEL-{}", suffix.to_uppercase());
        let claim_id = format!("claim-cancel-{suffix}");
        let execution_id = format!("exec-cancel-{suffix}");
        create_in_progress_goal_with_rounds(&refine_dir, &goal_id, 1);
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_workflow_agent(&supervisor, &goal_id, &claim_id, &execution_id, 0);
        reserve_workflow_capacity(&runtime_root, &claim_id);
        if cancel_before_stop {
            FileWorkItemService::new(&refine_dir)
                .cancel_goal_summary(&goal_id)
                .unwrap();
            let workflow = WorkflowEngine::new(&runtime_root);
            let mut state = workflow.load_state().unwrap();
            state.claims[0].state = WorkflowClaimState::Cancelled;
            state.version = state.version.saturating_add(1);
            workflow
                .persist_state_preserving_policy_locked(&state)
                .unwrap();
        }
        let hook_refine_dir = refine_dir.clone();
        let hook_goal_id = goal_id.clone();
        let mut control = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir);
        if !cancel_before_stop {
            control = control.with_post_exit_hook(move || {
                FileWorkItemService::new(&hook_refine_dir)
                    .cancel_goal_summary(&hook_goal_id)
                    .unwrap();
            });
        }

        let result = control.stop(&process.id, "terminate").unwrap();

        assert_eq!(result["stopped"], true);
        assert_eq!(result["termination_intent"], "explicit_cancellation");
        assert_eq!(result["goal"]["status"], "cancelled");
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
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .status,
            GoalStatus::Cancelled
        );

        remove_temp_dir(&temp_root);
    }
}

#[test]
fn interrupted_interactive_settlement_replay_is_superseded_by_explicit_cancellation() {
    let temp_root = unique_temp_dir("process-control-stop-replay-cancel");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-STOP-REPLAY-CANCEL";
    let claim_id = "claim-stop-replay-cancel";
    let execution_id = "exec-stop-replay-cancel";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);

    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_settlement_interruption(CancellationSettlementFailureStage::ClaimPersistence)
            .stop(&process.id, "terminate")
            .unwrap();
    }));
    assert!(interrupted.is_err());

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(execution_id)
        .unwrap();

    assert_eq!(result["cancelled"], true);
    assert_eq!(result["termination_intent"], "explicit_cancellation");
    assert_eq!(result["goal"]["status"], "cancelled");
    let journal: CancellationSettlementJournal = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(journal.schema_version, 7);
    assert_eq!(
        journal.requested_termination_intent,
        Some(TerminationIntent::InteractiveStop)
    );
    assert_eq!(
        journal.termination_intent,
        Some(TerminationIntent::ExplicitCancellation)
    );
    assert!(journal.goal_cancelled);
    assert!(!journal.goal_requeued);
    let receipt = process_outcome_receipt(&runtime_root, &process.id);
    assert_eq!(receipt["requested_termination_intent"], "interactive_stop");
    assert_eq!(receipt["termination_intent"], "explicit_cancellation");
    assert_eq!(receipt["goal_cancelled"], true);
    assert_eq!(receipt["goal_requeued"], false);

    remove_temp_dir(&temp_root);
}

#[test]
fn interrupted_explicit_settlement_replay_satisfies_later_interactive_stop() {
    let temp_root = unique_temp_dir("process-control-cancel-replay-stop");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-CANCEL-REPLAY-STOP";
    let claim_id = "claim-cancel-replay-stop";
    let execution_id = "exec-cancel-replay-stop";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);

    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_settlement_interruption(CancellationSettlementFailureStage::ClaimPersistence)
            .cancel_workflow_execution(execution_id)
            .unwrap();
    }));
    assert!(interrupted.is_err());

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .stop(&process.id, "terminate")
        .unwrap();

    assert_eq!(result["stopped"], true);
    assert_eq!(result["requested_termination_intent"], "interactive_stop");
    assert_eq!(result["termination_intent"], "explicit_cancellation");
    assert_eq!(result["goal"]["status"], "cancelled");
    assert_eq!(result["intent_superseded"], true);

    remove_temp_dir(&temp_root);
}

#[test]
fn superseded_stop_partial_failure_reports_durable_cancelled_truth() {
    let temp_root = unique_temp_dir("process-control-superseded-stop-partial");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-SUPERSEDED-STOP-PARTIAL";
    let claim_id = "claim-superseded-stop-partial";
    let execution_id = "exec-superseded-stop-partial";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);
    let hook_refine_dir = refine_dir.clone();

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_post_exit_hook(move || {
            FileWorkItemService::new(&hook_refine_dir)
                .cancel_goal_summary(goal_id)
                .unwrap();
        })
        .with_settlement_failure(CancellationSettlementFailureStage::ClaimPersistence)
        .stop(&process.id, "terminate")
        .unwrap_err();

    assert!(error.to_string().contains("partial outcome"), "{error}");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );
    let receipt = process_outcome_receipt(&runtime_root, &process.id);
    assert_eq!(receipt["state"], "partial_failure");
    assert_eq!(receipt["requested_termination_intent"], "interactive_stop");
    assert_eq!(receipt["termination_intent"], "explicit_cancellation");
    assert_eq!(receipt["goal_status"], "cancelled");
    assert_eq!(receipt["goal_cancelled"], true);
    assert_eq!(receipt["goal_requeued"], false);
    assert_eq!(receipt["claim_cancelled"], false);

    remove_temp_dir(&temp_root);
}

fn process_outcome_receipt(runtime_root: &Path, process_id: &str) -> Value {
    serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}
