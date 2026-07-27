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
            .with_settlement_interruption(CancellationSettlementFailureStage::AfterClaimPersistence)
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
