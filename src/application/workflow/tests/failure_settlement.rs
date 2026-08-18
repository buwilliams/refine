use super::*;
use crate::application::work_items::{BulkGoalSelection, BulkGoalUpdate, WorkflowAttemptAuthority};

#[test]
fn authoritative_failure_atomically_fails_goal_and_its_originating_round() {
    let temp_root = unique_temp_dir("authoritative-workflow-failure");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    prepare_todo_goal(&work_items, "GOAL1");
    let authority = claim_and_start(&work_items, "GOAL1");
    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);

    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            authority,
            "quality",
            &RefineError::Conflict("quality stopped".to_string()),
        ),
        Some(true)
    );

    let summary = work_items.show_goal_summary("GOAL1").unwrap();
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(summary.goal.status, GoalStatus::Failed);
    assert_eq!(detail["rounds"][0]["failure_category"], "quality");
    assert_eq!(detail["rounds"][0]["failure_message"], "quality stopped");
    assert_ne!(detail["rounds"][0]["failure_at"], "");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn undo_reclaim_of_cancelled_goal_supersedes_old_same_round_failure() {
    let temp_root = unique_temp_dir("cancelled-undo-reclaim-fence");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    prepare_todo_goal(&work_items, "GOAL1");
    let old_authority = claim_and_start(&work_items, "GOAL1");

    work_items.cancel_goal_summary("GOAL1").unwrap();
    let reopened = work_items.undo_goal_summary("GOAL1").unwrap();
    assert_eq!(reopened.goal.status, GoalStatus::Todo);
    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            old_authority,
            "workflow",
            &RefineError::Conflict("old worker stopped after reopen".to_string()),
        ),
        Some(false)
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    let replacement_authority = claim_and_start(&work_items, "GOAL1");
    assert_ne!(old_authority, replacement_authority);

    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            old_authority,
            "workflow",
            &RefineError::Conflict("old worker stopped late".to_string()),
        ),
        Some(false)
    );

    assert_active_replacement_is_clean(&work_items, "GOAL1", 0, replacement_authority);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn bulk_todo_reclaim_with_new_round_supersedes_old_failure() {
    let temp_root = unique_temp_dir("cancelled-bulk-new-round-reclaim-fence");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    prepare_todo_goal(&work_items, "GOAL1");
    let old_authority = claim_and_start(&work_items, "GOAL1");

    work_items.cancel_goal_summary("GOAL1").unwrap();
    let moved = work_items
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(moved.updated, 1);
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Replacement request")
        .unwrap();
    let replacement_authority = claim_and_start(&work_items, "GOAL1");
    assert_eq!(replacement_authority.round_idx, 1);

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            old_authority,
            "workflow",
            &RefineError::Conflict("old round stopped late".to_string()),
        ),
        Some(false)
    );

    assert_active_replacement_is_clean(&work_items, "GOAL1", 1, replacement_authority);
    fs::remove_dir_all(temp_root).unwrap();
}

fn prepare_todo_goal(work_items: &FileWorkItemService, goal_id: &str) {
    work_items
        .create_goal_summary("Failure fence", Some(goal_id))
        .unwrap();
    work_items
        .append_goal_round_summary(goal_id, "Reporter", "Original request")
        .unwrap();
    work_items
        .transition_goal_status(goal_id, GoalStatus::Todo)
        .unwrap();
}

fn claim_and_start(work_items: &FileWorkItemService, goal_id: &str) -> WorkflowAttemptAuthority {
    let (round_idx, revision, request) = work_items.authored_goal_commitment(goal_id).unwrap();
    let authority = work_items
        .claim_workflow_attempt(goal_id, GoalStatus::Todo, round_idx, revision, &request)
        .unwrap();
    work_items
        .advance_claimed_goal_status(goal_id, authority, GoalStatus::Todo, GoalStatus::Plan)
        .unwrap();
    authority
}

fn assert_active_replacement_is_clean(
    work_items: &FileWorkItemService,
    goal_id: &str,
    round_idx: usize,
    authority: WorkflowAttemptAuthority,
) {
    let summary = work_items.show_goal_summary(goal_id).unwrap();
    let detail = work_items.show_goal_detail(goal_id).unwrap();
    assert_eq!(summary.goal.status, GoalStatus::Plan);
    assert_eq!(summary.goal.round_count, round_idx + 1);
    assert_eq!(detail["rounds"][round_idx]["failure_category"], "");
    assert_eq!(detail["rounds"][round_idx]["failure_message"], "");
    assert_eq!(detail["rounds"][round_idx]["failure_at"], "");
    assert_eq!(
        detail["rounds"][round_idx]["workflow_attempt_authority"]["round_idx"],
        authority.round_idx
    );
    assert_eq!(
        detail["rounds"][round_idx]["workflow_attempt_authority"]["workflow_revision"],
        authority.workflow_revision
    );
}
