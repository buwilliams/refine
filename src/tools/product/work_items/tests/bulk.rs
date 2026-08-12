use super::*;

#[test]
fn file_work_item_service_bulk_updates_deletes_and_assigns_goals() {
    let temp_root = unique_temp_dir("work-item-bulk");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GOAL1", "Bulk one"),
        ("GOAL2", "Bulk two"),
        ("GOAL3", "Skip me"),
    ] {
        service.create_goal_summary(name, Some(id)).unwrap();
        service
            .append_goal_round_summary(id, "Original", "Prompt")
            .unwrap();
    }
    service
        .set_goal_status_unchecked("GOAL3", &GoalStatus::Qa)
        .unwrap();

    let status_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec![
                    "GOAL1".to_string(),
                    "GOAL2".to_string(),
                    "GOAL3".to_string(),
                ]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(status_result.updated, 2);
    assert_eq!(status_result.skipped, 1);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        service.show_goal_summary("GOAL3").unwrap().goal.status,
        GoalStatus::Qa
    );

    let reporter_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Reporter("Reviewer".to_string()),
        )
        .unwrap();
    assert_eq!(reporter_result.updated, 2);
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));

    let assignee_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Assignee("Assignee".to_string()),
        )
        .unwrap();
    assert_eq!(assignee_result.updated, 2);
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .assignee
            .as_deref(),
        Some("Assignee")
    );

    service
        .create_feature_summary("Bulk Feature", Some("FEA1"), None, None, None)
        .unwrap();
    let feature_assignee_result = service
        .bulk_update_features(
            BulkFeatureSelection {
                selected_ids: Some(vec!["FEA1".to_string()]),
                ..Default::default()
            },
            BulkFeatureUpdate::Assignee("Feature Reviewer".to_string()),
        )
        .unwrap();
    assert_eq!(feature_assignee_result.updated, 1);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .assignee
            .as_deref(),
        Some("Feature Reviewer")
    );
    let feature_reporter_result = service
        .bulk_update_features(
            BulkFeatureSelection {
                selected_ids: Some(vec!["FEA1".to_string()]),
                ..Default::default()
            },
            BulkFeatureUpdate::Reporter("Feature Reporter".to_string()),
        )
        .unwrap();
    assert_eq!(feature_reporter_result.updated, 1);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .reporter
            .as_deref(),
        Some("Feature Reporter")
    );
    let assign_result = service
        .bulk_assign_goals_to_feature(
            "FEA1",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(assign_result.updated, 2);
    assert_eq!(
        service.show_feature_summary("FEA1").unwrap().goal_ids,
        vec!["GOAL1", "GOAL2"]
    );

    let delete_result = service
        .bulk_delete_goals(BulkGoalSelection {
            selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(delete_result.deleted, 2);
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_bulk_status_allows_review_and_done() {
    let temp_root = unique_temp_dir("work-item-bulk-review-done");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for id in ["REVIEW_TARGET", "DONE_TARGET", "AUTOMATED"] {
        service.create_goal_summary(id, Some(id)).unwrap();
    }
    service
        .set_goal_status_unchecked("DONE_TARGET", &GoalStatus::Review)
        .unwrap();
    service
        .set_goal_status_unchecked("AUTOMATED", &GoalStatus::Qa)
        .unwrap();

    let review_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["REVIEW_TARGET".to_string(), "AUTOMATED".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("review".to_string()),
        )
        .unwrap();
    assert_eq!(review_result.updated, 1);
    assert_eq!(review_result.skipped, 1);
    assert_eq!(
        service
            .show_goal_summary("REVIEW_TARGET")
            .unwrap()
            .goal
            .status,
        GoalStatus::Review
    );

    let done_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec![
                    "REVIEW_TARGET".to_string(),
                    "DONE_TARGET".to_string(),
                    "AUTOMATED".to_string(),
                ]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("done".to_string()),
        )
        .unwrap();
    assert_eq!(done_result.updated, 2);
    assert_eq!(done_result.skipped, 1);
    assert_eq!(done_result.skipped_details[0].reason, "status:qa");
    for id in ["REVIEW_TARGET", "DONE_TARGET"] {
        assert_eq!(
            service.show_goal_summary(id).unwrap().goal.status,
            GoalStatus::Done
        );
    }
    assert_eq!(
        service.show_goal_summary("AUTOMATED").unwrap().goal.status,
        GoalStatus::Qa
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn bulk_review_and_done_revalidate_status_after_selection() {
    let temp_root = unique_temp_dir("work-item-bulk-status-selection-race");
    let target_root = temp_root.join("target");
    let refine_dir = target_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let cache_dir = runtime_root.join("cache");
    let service =
        FileWorkItemService::with_projection_cache(&refine_dir, &runtime_root, &cache_dir);
    for id in [
        "CLAIMED_AFTER_SELECTION",
        "AUTOMATED_AFTER_SELECTION",
        "MOVED_AFTER_SELECTION",
    ] {
        service.create_goal_summary(id, Some(id)).unwrap();
        service
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
    }
    crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir)
        .create("remote-node")
        .unwrap();

    let race_service = service.clone();
    let status_race_service = service
        .clone()
        .with_after_bulk_goal_selection_hook(move || {
            race_service
                .append_goal_round_summary("CLAIMED_AFTER_SELECTION", "Reporter", "Implement")
                .unwrap();
            race_service
                .advance_automated_goal_status("CLAIMED_AFTER_SELECTION", GoalStatus::InProgress)
                .unwrap();
        });
    let review_result = status_race_service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["CLAIMED_AFTER_SELECTION".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("review".to_string()),
        )
        .unwrap();
    assert_eq!(review_result.updated, 0);
    assert_eq!(review_result.skipped, 1);
    assert_eq!(
        review_result.skipped_details[0].reason,
        "status:in-progress"
    );
    assert_eq!(
        service
            .show_goal_summary("CLAIMED_AFTER_SELECTION")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    let done_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["CLAIMED_AFTER_SELECTION".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("done".to_string()),
        )
        .unwrap();
    assert_eq!(done_result.updated, 0);
    assert_eq!(done_result.skipped, 1);
    assert_eq!(done_result.skipped_details[0].reason, "status:in-progress");
    assert_eq!(
        service
            .show_goal_summary("CLAIMED_AFTER_SELECTION")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    let status_mutator = FileWorkItemService::new(&refine_dir);
    let status_race_service = service
        .clone()
        .with_after_bulk_goal_selection_hook(move || {
            status_mutator
                .set_goal_status_unchecked("AUTOMATED_AFTER_SELECTION", &GoalStatus::Qa)
                .unwrap();
        });
    let automated_result = status_race_service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["AUTOMATED_AFTER_SELECTION".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("done".to_string()),
        )
        .unwrap();
    assert_eq!(automated_result.updated, 0);
    assert_eq!(automated_result.skipped, 1);
    assert_eq!(automated_result.skipped_details[0].reason, "status:qa");
    assert_eq!(
        service
            .show_goal_summary("AUTOMATED_AFTER_SELECTION")
            .unwrap()
            .goal
            .status,
        GoalStatus::Qa
    );

    let node_mutator = FileWorkItemService::new(&refine_dir);
    let node_race_service = service
        .clone()
        .with_after_bulk_goal_selection_hook(move || {
            node_mutator
                .transfer_goal_to_node("remote-node", "MOVED_AFTER_SELECTION")
                .unwrap();
        });
    let moved_result = node_race_service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["MOVED_AFTER_SELECTION".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("review".to_string()),
        )
        .unwrap();
    assert_eq!(moved_result.updated, 0);
    assert_eq!(moved_result.skipped, 1);
    assert_eq!(moved_result.skipped_details[0].reason, "node:remote-node");
    let moved = service.show_goal_summary("MOVED_AFTER_SELECTION").unwrap();
    assert_eq!(moved.goal.status, GoalStatus::Todo);
    assert_eq!(moved.goal.node_id.as_deref(), Some("remote-node"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_bulk_deletes_features() {
    let temp_root = unique_temp_dir("work-item-feature-bulk-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Bulk Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service.create_goal_summary("First", Some("GOAL1")).unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();

    let deleted = service
        .bulk_delete_features(BulkFeatureSelection {
            selected_ids: Some(vec!["FEA1".to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(deleted.deleted, 1);
    assert_eq!(deleted.ids, vec!["FEA1"]);
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}
