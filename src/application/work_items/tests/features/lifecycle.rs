use super::*;

#[test]
fn file_work_item_service_cancels_and_deletes_features_through_goal_paths() {
    let temp_root = unique_temp_dir("work-item-feature-cancel-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GOAL1", "Backlog Goal"),
        ("GOAL2", "Todo Goal"),
        ("GOAL3", "Done Goal"),
    ] {
        service.create_goal_summary(name, Some(id)).unwrap();
    }
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    for goal_id in ["GOAL1", "GOAL2", "GOAL3"] {
        service.assign_goal_to_feature("FEA1", goal_id).unwrap();
    }
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    service
        .set_goal_status_unchecked("GOAL3", &GoalStatus::Done)
        .unwrap();

    let cancelled = service.cancel_feature_summary("FEA1").unwrap();
    assert_eq!(cancelled.rollup.status, GoalStatus::Done);
    assert_eq!(cancelled.rollup.cancelled_count, 2);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Cancelled
    );
    assert_eq!(
        service.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Cancelled
    );
    assert_eq!(
        service.show_goal_summary("GOAL3").unwrap().goal.status,
        GoalStatus::Done
    );

    service.delete_feature_record("FEA1").unwrap();
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL3/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}
