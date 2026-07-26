use super::*;

#[test]
fn file_automation_promotes_todo_goals_and_starts_executions() {
    let temp_root = unique_temp_dir("automation");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Queued", Some("GOAL1"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .create_goal_summary("Backlog", Some("GOAL2"))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    assert_eq!(automation.promote().unwrap(), 0);
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "GOAL1");

    let execution_id = automation.start_claim(&state.claims[0].claim_id).unwrap();
    assert!(execution_id.starts_with("exec-"));
    let state = automation.load_state().unwrap();
    assert_eq!(
        state.claims[0].execution_id.as_deref(),
        Some(execution_id.as_str())
    );
    assert_eq!(state.claims[0].state, WorkflowClaimState::Running);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_auto_promotes_backlog_goals_when_configured() {
    let temp_root = unique_temp_dir("automation-backlog-promote");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Instant Backlog", Some("GOAL1"))
        .unwrap();
    work_items
        .create_goal_summary("Never Backlog", Some("GOAL2"))
        .unwrap();
    let settings = FileSettingsService::new(&refine_dir);
    settings
        .update(&json!({"backlog_promote_after_seconds": "-1"}))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 0);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Backlog
    );

    settings
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    assert_eq!(automation.promote().unwrap(), 2);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Todo
    );
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 2);
    let mut claimed_goal_ids = state
        .claims
        .iter()
        .map(|claim| claim.goal_id.as_str())
        .collect::<Vec<_>>();
    claimed_goal_ids.sort_unstable();
    assert_eq!(claimed_goal_ids, vec!["GOAL1", "GOAL2"]);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_promotes_all_ordered_feature_backlog_goals() {
    let temp_root = unique_temp_dir("automation-feature-backlog-promote");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_feature_summary("Imported Feature", Some("FEA1"), None, None, None)
        .unwrap();
    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items.assign_goal_to_feature("FEA1", id).unwrap();
        work_items.order_goal_in_feature("FEA1", id).unwrap();
    }
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote_backlog_to_todo().unwrap(), 3);
    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        assert_eq!(
            work_items.show_goal_summary(id).unwrap().goal.status,
            GoalStatus::Todo
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_blocks_lower_priority_work_behind_higher_priority_goals() {
    let temp_root = unique_temp_dir("automation-priority-band");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"parallel_run_cap": 3}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for (id, priority) in [("LOW", "low"), ("MEDIUM", "medium"), ("HIGH", "high")] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items
            .update_goal_metadata_summary(id, None, Some(priority), None, None)
            .unwrap();
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert!(automation.claim("MEDIUM").is_err());
    assert!(automation.claim("LOW").is_err());
    assert_eq!(automation.promote().unwrap(), 1);
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "HIGH");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_respects_feature_order_on_promote_claim_and_start() {
    let temp_root = unique_temp_dir("automation-feature-order");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let claim_runtime_root = temp_root.join("run/8081");
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2
        }))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_feature_summary("Feature", Some("FEAT1"), None, None, None)
        .unwrap();
    for id in ["FIRST", "SECOND", "UNORDERED"] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
        work_items.assign_goal_to_feature("FEAT1", id).unwrap();
    }
    for id in ["FIRST", "SECOND"] {
        work_items.order_goal_in_feature("FEAT1", id).unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert!(automation.claim("SECOND").is_err());
    assert_eq!(automation.promote().unwrap(), 2);
    let state = automation.load_state().unwrap();
    let claimed_goal_ids = state
        .claims
        .iter()
        .map(|claim| claim.goal_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(claimed_goal_ids, vec!["FIRST", "UNORDERED"]);

    for status in [
        GoalStatus::InProgress,
        GoalStatus::ReadyMerge,
        GoalStatus::Build,
        GoalStatus::Review,
    ] {
        work_items
            .advance_automated_goal_status("FIRST", status)
            .unwrap();
    }
    let claim_automation = WorkflowEngine::with_target_root(&claim_runtime_root, &target_root);
    assert_eq!(claim_automation.promote().unwrap(), 2);
    let state = claim_automation.load_state().unwrap();
    let second_claim = state
        .claims
        .iter()
        .find(|claim| claim.goal_id == "SECOND")
        .map(|claim| claim.claim_id.clone())
        .unwrap();
    let rejected_bulk_reopen = work_items
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["FIRST".to_string()]),
                ..Default::default()
            },
            crate::tools::product::work_items::BulkGoalUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(rejected_bulk_reopen.updated, 0);
    assert_eq!(rejected_bulk_reopen.skipped, 1);
    assert_eq!(
        work_items.show_goal_summary("FIRST").unwrap().goal.status,
        GoalStatus::Review
    );
    claim_automation.start_claim(&second_claim).unwrap();

    fs::remove_dir_all(temp_root).unwrap();
}
