use super::*;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};

#[test]
fn bulk_cancellation_composes_process_operation_claim_capacity_and_inactive_settlement() {
    let temp_root = unique_temp_dir("process-control-bulk-lifecycle");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let running_goal = "GOAL-BULK-RUNNING";
    let claimed_goal = "GOAL-BULK-CLAIMED";
    let inactive_goal = "GOAL-BULK-INACTIVE";
    let done_goal = "GOAL-BULK-DONE";
    let claim_id = "claim-bulk-running";
    let execution_id = "exec-bulk-running";
    create_in_progress_goal_with_rounds(&refine_dir, running_goal, 1);
    let work_items = FileWorkItemService::new(&refine_dir);
    for (goal_id, status) in [
        (claimed_goal, GoalStatus::Todo),
        (inactive_goal, GoalStatus::Review),
        (done_goal, GoalStatus::Done),
    ] {
        work_items
            .create_goal_summary(goal_id, Some(goal_id))
            .unwrap();
        work_items
            .set_goal_status_unchecked(goal_id, &status)
            .unwrap();
    }
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, running_goal, claim_id, execution_id, 0);
    write_workflow_state(
        &runtime_root,
        json!([
            {
                "claim_id": claim_id,
                "goal_id": running_goal,
                "execution_id": execution_id,
                "round_idx": 0,
                "state": "running",
                "created_at": "2026-07-23T00:00:00Z",
                "updated_at": "2026-07-23T00:00:00Z"
            },
            {
                "claim_id": "claim-bulk-unstarted",
                "goal_id": claimed_goal,
                "state": "claimed",
                "created_at": "2026-07-23T00:00:00Z",
                "updated_at": "2026-07-23T00:00:00Z"
            }
        ]),
    );
    reserve_workflow_capacity(&runtime_root, claim_id);
    let operation = FileOperationRegistry::new(&runtime_root)
        .register_with_request("bulk-test", json!({"execution_id": execution_id}))
        .unwrap();

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(
                [running_goal, claimed_goal, inactive_goal, done_goal]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            ),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(result.updated, 3);
    assert_eq!(
        result.ids,
        vec![
            claimed_goal.to_string(),
            inactive_goal.to_string(),
            running_goal.to_string()
        ]
    );
    assert_eq!(result.failed, 0);
    assert_eq!(result.skipped, 1);
    assert_eq!(result.skipped_details[0].id, done_goal);
    assert_eq!(result.skipped_details[0].reason, "status:done");
    assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());
    assert_eq!(
        FileOperationRegistry::new(&runtime_root)
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Cancelled
    );
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    assert!(
        state
            .claims
            .iter()
            .all(|claim| { matches!(claim.state, WorkflowClaimState::Cancelled) })
    );
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    for goal_id in [running_goal, claimed_goal, inactive_goal] {
        assert_eq!(
            work_items.show_goal_summary(goal_id).unwrap().goal.status,
            GoalStatus::Cancelled
        );
    }
    assert_eq!(
        work_items.show_goal_summary(done_goal).unwrap().goal.status,
        GoalStatus::Done
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
    assert_eq!(receipt["state"], "completed");
    assert_eq!(receipt["confirmed_exit"], true);
    assert_eq!(receipt["goal_cancelled"], true);
    assert_eq!(receipt["claim_cancelled"], true);

    remove_temp_dir(&temp_root);
}

#[test]
fn bulk_cancellation_reports_per_goal_failure_and_continues_safe_peers() {
    let temp_root = unique_temp_dir("process-control-bulk-partial");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let failed_goal = "GOAL-BULK-FAIL";
    let safe_goal = "GOAL-BULK-SAFE";
    create_in_progress_goal_with_rounds(&refine_dir, failed_goal, 1);
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary(safe_goal, Some(safe_goal))
        .unwrap();
    work_items
        .set_goal_status_unchecked(safe_goal, &GoalStatus::Failed)
        .unwrap();
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-bulk-missing-process",
            "goal_id": failed_goal,
            "execution_id": "exec-bulk-missing-process",
            "round_idx": 0,
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );
    reserve_workflow_capacity(&runtime_root, "claim-bulk-missing-process");

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(vec![failed_goal.to_string(), safe_goal.to_string()]),
            ..Default::default()
        })
        .unwrap();

    assert_eq!(result.updated, 1);
    assert_eq!(result.ids, vec![safe_goal]);
    assert_eq!(result.failed, 1);
    assert_eq!(result.failures[0]["id"], failed_goal);
    assert_eq!(result.failures[0]["error"]["code"], "conflict");
    assert!(
        result.failures[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no managed-process record")
    );
    assert_eq!(
        work_items
            .show_goal_summary(failed_goal)
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert_eq!(
        work_items.show_goal_summary(safe_goal).unwrap().goal.status,
        GoalStatus::Cancelled
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
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .len(),
        1
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn bulk_cancellation_replays_interrupted_settlement_and_revalidates_done_race() {
    let temp_root = unique_temp_dir("process-control-bulk-replay-race");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let replay_goal = "GOAL-BULK-REPLAY";
    let race_goal = "GOAL-BULK-RACE";
    let moved_goal = "GOAL-BULK-MOVED";
    let claim_id = "claim-bulk-replay";
    let execution_id = "exec-bulk-replay";
    create_in_progress_goal_with_rounds(&refine_dir, replay_goal, 1);
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary(race_goal, Some(race_goal))
        .unwrap();
    work_items
        .create_goal_summary(moved_goal, Some(moved_goal))
        .unwrap();
    FileNodeRegistryService::new(&refine_dir)
        .create("remote-node")
        .unwrap();
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, replay_goal, claim_id, execution_id, 0);
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": claim_id,
            "goal_id": replay_goal,
            "execution_id": execution_id,
            "round_idx": 0,
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );
    reserve_workflow_capacity(&runtime_root, claim_id);

    let interrupted = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_settlement_interruption(CancellationSettlementFailureStage::ClaimPersistence);
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        interrupted.bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(vec![replay_goal.to_string()]),
            ..Default::default()
        })
    }));
    assert!(first.is_err());
    assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());

    let replayed = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(vec![replay_goal.to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(replayed.updated, 1);
    assert_eq!(replayed.failed, 0);
    assert_eq!(
        work_items
            .show_goal_summary(replay_goal)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
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

    let race_mutator = work_items.clone();
    let raced = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_after_bulk_goal_selection_hook(move || {
            race_mutator
                .set_goal_status_unchecked(race_goal, &GoalStatus::Done)
                .unwrap();
            race_mutator
                .transfer_goal_to_node("remote-node", moved_goal)
                .unwrap();
        })
        .bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(vec![race_goal.to_string(), moved_goal.to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(raced.updated, 0);
    assert_eq!(raced.skipped, 2);
    assert_eq!(raced.skipped_details[0].id, moved_goal);
    assert_eq!(raced.skipped_details[0].reason, "node:remote-node");
    assert_eq!(raced.skipped_details[1].id, race_goal);
    assert_eq!(raced.skipped_details[1].reason, "status:done");
    assert_eq!(
        work_items.show_goal_summary(race_goal).unwrap().goal.status,
        GoalStatus::Done
    );
    assert_eq!(
        work_items
            .show_goal_summary(moved_goal)
            .unwrap()
            .goal
            .node_id
            .as_deref(),
        Some("remote-node")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn cancel_goal_without_active_claim_uses_explicit_cancellation_intent() {
    let temp_root = unique_temp_dir("process-control-goal-cancel-no-claim");
    let runtime_root = temp_root.join("run/8080");
    let (target_root, refine_dir) = init_git_target(&temp_root);
    let goal_id = "GOAL-CANCEL-NO-CLAIM";
    let branch = "refine/cancel-no-claim";
    create_in_progress_goal(&refine_dir, goal_id);
    let worktree = add_test_worktree(&target_root, branch, "cancel-no-claim");
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent_with_metadata(
        &supervisor,
        goal_id,
        None,
        Map::from_iter([(
            "worktree".to_string(),
            json!({"path": worktree, "branch": branch}),
        )]),
    );

    let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_goal(goal_id)
        .unwrap();

    assert_eq!(result["cancelled"], true);
    assert_eq!(result["termination_intent"], "explicit_cancellation");
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
    assert!(worktree.exists());
    assert_branch_exists(&target_root, branch);
    let receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["termination_intent"], "explicit_cancellation");
    assert_eq!(receipt["goal_cancelled"], true);
    assert_eq!(receipt["goal_requeued"], false);
    assert_eq!(receipt["worktree_retention"]["retained"], true);

    remove_temp_dir(&temp_root);
}

#[test]
fn bulk_cancellation_and_interactive_stop_are_monotonic_in_both_orderings() {
    let temp_root = unique_temp_dir("process-control-bulk-stop-orderings");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let control = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir);

    let stop_first_goal = "GOAL-STOP-THEN-BULK";
    create_in_progress_goal(&refine_dir, stop_first_goal);
    let stop_first_process = launch_agent(&supervisor, stop_first_goal, None);
    let stopped = control.stop(&stop_first_process.id, "terminate").unwrap();
    assert_eq!(stopped["goal"]["status"], "todo");
    let bulk_after_stop = control
        .bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(vec![stop_first_goal.to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(bulk_after_stop.updated, 1);
    assert_eq!(
        work_items
            .show_goal_summary(stop_first_goal)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );

    let bulk_first_goal = "GOAL-BULK-THEN-STOP";
    create_in_progress_goal(&refine_dir, bulk_first_goal);
    work_items.cancel_goal_summary(bulk_first_goal).unwrap();
    let bulk_first_process = launch_agent(&supervisor, bulk_first_goal, None);
    let bulk_before_stop = control
        .bulk_cancel_goals(BulkGoalSelection {
            selected_ids: Some(vec![bulk_first_goal.to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(bulk_before_stop.updated, 1);
    assert!(managed_pid_is_alive(bulk_first_process.pid.unwrap()).unwrap());
    let stopped_after_bulk = control.stop(&bulk_first_process.id, "terminate").unwrap();
    assert_eq!(stopped_after_bulk["stopped"], true);
    assert_eq!(
        stopped_after_bulk["termination_intent"],
        "explicit_cancellation"
    );
    assert_eq!(stopped_after_bulk["goal"]["status"], "cancelled");
    assert_eq!(
        work_items
            .show_goal_summary(bulk_first_goal)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn cancel_goal_partial_failure_never_reports_cancelled_success() {
    let temp_root = unique_temp_dir("process-control-goal-cancel-partial");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-CANCEL-PARTIAL";
    create_in_progress_goal(&refine_dir, goal_id);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent(&supervisor, goal_id, None);

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_settlement_failure(CancellationSettlementFailureStage::GoalPersistence)
        .cancel_goal(goal_id)
        .unwrap_err();

    assert!(error.to_string().contains("partial outcome"), "{error}");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
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
    assert_eq!(receipt["termination_intent"], "explicit_cancellation");
    assert_eq!(receipt["goal_cancelled"], false);
    assert_eq!(receipt["goal_requeued"], false);

    remove_temp_dir(&temp_root);
}
