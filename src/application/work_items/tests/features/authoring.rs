use super::*;

#[test]
fn file_work_item_service_exposes_failed_feature_blocking_notice_on_goal_detail() {
    let temp_root = unique_temp_dir("work-item-feature-blocking-notice");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL1").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL2").unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();

    let detail = service.show_goal_detail("GOAL1").unwrap();
    let notice = &detail["feature_blocking_notice"];
    assert_eq!(notice["feature_id"], "FEA1");
    assert_eq!(notice["blocking_goal_id"], "GOAL1");
    assert_eq!(notice["blocked_count"], 1);
    assert_eq!(notice["blocked_goal_ids"], json!(["GOAL2"]));
    assert!(
        notice["message"]
            .as_str()
            .unwrap_or("")
            .contains("blocking the next Goal")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_transfers_features_as_node_owned_units() {
    let temp_root = unique_temp_dir("work-item-feature-transfer");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.create_goal_summary("First", Some("GOAL1")).unwrap();
    service
        .create_goal_summary("Second", Some("GOAL2"))
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();

    let direct_goal = service
        .transfer_goal_to_node("remote-node", "GOAL1")
        .unwrap_err();
    assert!(
        direct_goal
            .to_string()
            .contains("transfer the Feature instead"),
        "{direct_goal}"
    );
    let bulk = service
        .bulk_transfer_goals_to_node(
            "remote-node",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bulk.updated, 0);
    assert_eq!(bulk.skipped, 2);
    assert_eq!(bulk.skipped_details[0].reason, "feature:FEA1");

    let bulk_feature = service
        .bulk_transfer_features_to_node(
            "remote-node",
            BulkFeatureSelection {
                selected_ids: Some(vec!["FEA1".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bulk_feature.updated, 3);
    assert_eq!(bulk_feature.ids, vec!["FEA1", "GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("remote-node")
    );

    service.transfer_feature_to_node("default", "FEA1").unwrap();

    let transferred = service
        .transfer_feature_to_node("remote-node", "FEA1")
        .unwrap();
    assert_eq!(transferred.updated, 3);
    assert_eq!(transferred.ids, vec!["FEA1", "GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("remote-node")
    );
    for goal_id in ["GOAL1", "GOAL2"] {
        assert_eq!(
            service
                .show_goal_summary(goal_id)
                .unwrap()
                .goal
                .node_id
                .as_deref(),
            Some("remote-node")
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_rejects_feature_transfer_with_active_member_goal() {
    let temp_root = unique_temp_dir("work-item-feature-transfer-active");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service
        .create_goal_summary("Active", Some("GOAL1"))
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();

    let err = service
        .transfer_feature_to_node("remote-node", "FEA1")
        .unwrap_err();
    assert!(err.to_string().contains("status:plan"), "{err}");
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("default")
    );
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .node_id
            .as_deref(),
        Some("default")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_skips_feature_and_active_goals_and_honors_dry_run() {
    let temp_root = unique_temp_dir("distribute-skips");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("node-a").unwrap();
    service
        .create_goal_summary("In feature", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Active", Some("GOAL2"))
        .unwrap();
    service.create_goal_summary("Free", Some("GOAL3")).unwrap();
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();

    service
        .append_goal_round_summary("GOAL2", "Reporter", "Implement")
        .unwrap();
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL2", GoalStatus::Plan)
        .unwrap();
    let targets = vec!["node-a".to_string()];
    let result = service
        .distribute_goals_across_nodes(&targets, false, true)
        .unwrap();

    assert_eq!(result.strategy, "fill");
    assert!(result.dry_run);
    assert_eq!(result.moved, 1);
    assert_eq!(result.moves[0].goal_id, "GOAL3");
    assert_eq!(result.skipped, 1);
    let reasons: Vec<&str> = result
        .skipped_details
        .iter()
        .map(|detail| detail.reason.as_str())
        .collect();
    assert!(reasons.contains(&"feature:FEA1"));
    let free_goal = service.show_goal_summary("GOAL3").unwrap();
    assert_eq!(
        free_goal
            .goal
            .node_id
            .unwrap_or_else(|| "default".to_string()),
        "default"
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_goal_authoring_centralizes_create_review_edit_and_placement() {
    let temp_root = unique_temp_dir("feature-goal-authoring");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service
        .create_goal_summary("Foundation", Some("GOAL1"))
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL1").unwrap();

    let created = service
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                prompt: "Implement the shared Feature Goal operation".to_string(),
                reporter: "Buddy".to_string(),
                priority: "high".to_string(),
                placement: FeatureGoalPlacement::After("GOAL1".to_string()),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap();
    assert!(created.created);
    let goal = created.goal.unwrap();
    assert_eq!(goal.name, "Implement the shared Feature Goal operation");
    assert_eq!(goal.priority, GoalPriority::High);
    assert_eq!(goal.feature_order, Some(2));
    let goal_id = goal.id;
    assert_eq!(
        service.show_goal_detail(&goal_id).unwrap()["rounds"][0]["prompt"],
        "Implement the shared Feature Goal operation"
    );

    let goal_path = refine_dir
        .join("goals")
        .join(&goal_id[..2])
        .join(&goal_id[2..])
        .join("goal.json");
    let mut review_goal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    review_goal["status"] = json!("review");
    fs::write(&goal_path, serde_json::to_vec_pretty(&review_goal).unwrap()).unwrap();
    let review_summary = service.show_goal_summary(&goal_id).unwrap();
    assert!(FileWorkItemService::feature_goal_authoring_capability(&review_summary).editable);

    let edited = service
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                goal_id: Some(goal_id.clone()),
                name: Some("Reviewed authoring".to_string()),
                prompt: "Revise the prompt while review is active".to_string(),
                reporter: "Buddy".to_string(),
                priority: "medium".to_string(),
                placement: FeatureGoalPlacement::First,
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap();
    assert!(!edited.created);
    let edited = edited.goal.unwrap();
    assert_eq!(edited.status, GoalStatus::Review);
    assert_eq!(edited.name, "Reviewed authoring");
    assert_eq!(edited.feature_order, Some(1));
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .feature_order,
        Some(2)
    );
    assert_eq!(
        service.show_goal_detail(&goal_id).unwrap()["rounds"][0]["prompt"],
        "Revise the prompt while review is active"
    );

    let mut done_goal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    done_goal["status"] = json!("done");
    fs::write(&goal_path, serde_json::to_vec_pretty(&done_goal).unwrap()).unwrap();
    let done_summary = service.show_goal_summary(&goal_id).unwrap();
    assert!(!FileWorkItemService::feature_goal_authoring_capability(&done_summary).editable);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_goal_authoring_reports_duplicates_and_validates_before_writes() {
    let temp_root = unique_temp_dir("feature-goal-authoring-validation");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    let request = FeatureGoalAuthoringRequest {
        prompt: "Same prompt".to_string(),
        reporter: "Buddy".to_string(),
        priority: "low".to_string(),
        ..FeatureGoalAuthoringRequest::default()
    };
    service
        .author_feature_goal("FEA1", request.clone())
        .unwrap();
    let duplicate = service.author_feature_goal("FEA1", request).unwrap();
    assert!(duplicate.requires_duplicate_decision);
    assert_eq!(duplicate.duplicate.unwrap().prompt, "Same prompt");
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);

    let invalid = service.author_feature_goal(
        "FEA1",
        FeatureGoalAuthoringRequest {
            prompt: "A different prompt".to_string(),
            reporter: "Buddy".to_string(),
            priority: "low".to_string(),
            placement: FeatureGoalPlacement::After("MISSING".to_string()),
            ..FeatureGoalAuthoringRequest::default()
        },
    );
    assert!(invalid.is_err());
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_goal_authoring_keeps_ordinary_and_feature_creation_in_parity() {
    let ordinary_root = unique_temp_dir("ordinary-goal-authoring-parity");
    let feature_root = unique_temp_dir("feature-goal-authoring-parity");
    let ordinary = FileWorkItemService::new(ordinary_root.join(".refine"));
    let feature = FileWorkItemService::new(feature_root.join(".refine"));
    feature
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    feature
        .create_goal_summary("Foundation", Some("FOUNDATION"))
        .unwrap();
    feature
        .assign_goal_to_feature("FEA1", "FOUNDATION")
        .unwrap();
    feature.order_goal_in_feature("FEA1", "FOUNDATION").unwrap();

    let generated_prompt =
        "  Build   one shared Goal authoring capability with deterministic names.  ";
    let ordinary_generated = ordinary
        .author_goal(GoalAuthoringRequest {
            prompt: generated_prompt.to_string(),
            reporter: "Buddy".to_string(),
            assignee: Some("Alice".to_string()),
            priority: "high".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap()
        .goal
        .unwrap();
    let feature_generated = feature
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                prompt: generated_prompt.to_string(),
                reporter: "Buddy".to_string(),
                assignee: Some("Alice".to_string()),
                priority: "high".to_string(),
                placement: FeatureGoalPlacement::After("FOUNDATION".to_string()),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap()
        .goal
        .unwrap();
    assert_eq!(ordinary_generated.name, feature_generated.name);
    assert_eq!(ordinary_generated.priority, feature_generated.priority);
    assert_eq!(ordinary_generated.reporter, feature_generated.reporter);
    assert_eq!(ordinary_generated.assignee, feature_generated.assignee);
    assert_eq!(feature_generated.feature_order, Some(2));
    for (service, goal_id) in [
        (&ordinary, ordinary_generated.id.as_str()),
        (&feature, feature_generated.id.as_str()),
    ] {
        let detail = service.show_goal_detail(goal_id).unwrap();
        assert_eq!(detail["rounds"][0]["reporter"], "Buddy");
        assert_eq!(detail["rounds"][0]["assignee"], "Alice");
        assert_eq!(detail["rounds"][0]["prompt"], generated_prompt.trim());
    }

    let ordinary_explicit = ordinary
        .author_goal(GoalAuthoringRequest {
            name: Some("Explicit shared name".to_string()),
            prompt: "Ordinary explicit prompt".to_string(),
            reporter: "Buddy".to_string(),
            priority: "low".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap()
        .goal
        .unwrap();
    let feature_explicit = feature
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                name: Some("Explicit shared name".to_string()),
                prompt: "Feature explicit prompt".to_string(),
                reporter: "Buddy".to_string(),
                priority: "low".to_string(),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap()
        .goal
        .unwrap();
    assert_eq!(ordinary_explicit.name, "Explicit shared name");
    assert_eq!(feature_explicit.name, ordinary_explicit.name);

    let ordinary_before = ordinary.list_goal_summaries().unwrap().len();
    let feature_before = feature.list_goal_summaries().unwrap().len();
    let ordinary_invalid = ordinary
        .author_goal(GoalAuthoringRequest {
            name: Some("Invalid".to_string()),
            reporter: "Bad\nReporter".to_string(),
            priority: "low".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap_err();
    let feature_invalid = feature
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                name: Some("Invalid".to_string()),
                prompt: "Invalid reporter".to_string(),
                reporter: "Bad\nReporter".to_string(),
                priority: "low".to_string(),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap_err();
    assert_eq!(ordinary_invalid.to_string(), feature_invalid.to_string());
    assert_eq!(
        ordinary.list_goal_summaries().unwrap().len(),
        ordinary_before
    );
    assert_eq!(feature.list_goal_summaries().unwrap().len(), feature_before);

    fs::remove_dir_all(ordinary_root).unwrap();
    fs::remove_dir_all(feature_root).unwrap();
}
