use super::*;

#[test]
fn file_work_item_service_creates_features_and_updates_goal_membership() {
    let temp_root = unique_temp_dir("work-item-feature");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();

    let feature = service
        .create_feature_summary(
            "Feature A",
            Some("FEA1"),
            Some("desc"),
            Some("Reporter"),
            Some("Reviewer"),
        )
        .unwrap();
    assert_eq!(feature.feature.id, "FEA1");
    assert_eq!(feature.feature.assignee.as_deref(), Some("Reviewer"));
    assert!(refine_dir.join("features/FE/A1/feature.json").exists());

    let feature = service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1"]);
    let feature = service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL2")
            .unwrap()
            .goal
            .feature_order,
        None
    );

    let feature = service.unorder_goal_in_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .feature_order,
        None
    );
    assert_eq!(
        service
            .show_goal_summary("GOAL2")
            .unwrap()
            .goal
            .feature_order,
        None
    );

    let feature = service.order_goal_in_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .feature_order,
        Some(1)
    );

    let feature = service.remove_goal_from_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL2")
            .unwrap()
            .goal
            .feature_order,
        None
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_reorders_and_moves_feature_workflow() {
    let temp_root = unique_temp_dir("work-item-feature-workflow");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();
    service
        .create_goal_summary("Goal C", Some("GOAL3"))
        .unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL3").unwrap();
    for goal_id in ["GOAL1", "GOAL2", "GOAL3"] {
        service.order_goal_in_feature("FEA1", goal_id).unwrap();
    }

    let reordered = service.reorder_goal_in_feature("FEA1", "GOAL3", 1).unwrap();
    assert_eq!(reordered.goal_ids, vec!["GOAL3", "GOAL1", "GOAL2"]);
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    let moved = service
        .move_feature_workflow("FEA1", GoalStatus::Backlog)
        .unwrap();
    assert_eq!(moved.rollup.status, GoalStatus::Backlog);
    assert_eq!(
        service.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Backlog
    );

    fs::remove_dir_all(temp_root).unwrap();
}
