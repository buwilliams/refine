use super::*;

#[test]
fn restart_recovery_preserves_goal_state_and_removes_retired_execution_files() {
    let temp_root = unique_temp_dir("execution-ownership-recovery");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Restartable", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Continue idempotently")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();

    fs::create_dir_all(runtime_root.join("operations/.workflow-cancellations")).unwrap();
    for name in [
        "workflow-automation-state.json",
        ".workflow-automation-state.lock",
        "agent-capacity-state.json",
        ".agent-capacity.lock",
        ".workflow-process-registration.lock",
    ] {
        fs::create_dir_all(&runtime_root).unwrap();
        fs::write(runtime_root.join(name), b"legacy").unwrap();
    }
    fs::write(
        runtime_root.join("operations/.workflow-cancellations/GOAL1.json"),
        b"{}",
    )
    .unwrap();

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        workflow.recover_interrupted_goals("test restart").unwrap(),
        1
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::InProgress
    );
    for name in [
        "workflow-automation-state.json",
        ".workflow-automation-state.lock",
        "agent-capacity-state.json",
        ".agent-capacity.lock",
        ".workflow-process-registration.lock",
        "operations/.workflow-cancellations",
    ] {
        assert!(!runtime_root.join(name).exists(), "{name} was not removed");
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn backlog_promotion_changes_only_goal_state_and_does_not_create_worktrees() {
    let temp_root = unique_temp_dir("state-only-promotion");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for index in 0..8 {
        let id = format!("GOAL{index}");
        work_items.create_goal_summary(&id, Some(&id)).unwrap();
    }

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(workflow.promote().unwrap(), 8);
    for index in 0..8 {
        let id = format!("GOAL{index}");
        assert_eq!(
            work_items.show_goal_summary(&id).unwrap().goal.status,
            GoalStatus::Todo
        );
    }
    assert!(!target_root.join(".git/refine-worktrees").exists());

    fs::remove_dir_all(temp_root).unwrap();
}
