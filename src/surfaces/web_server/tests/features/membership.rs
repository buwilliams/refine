use super::*;

#[test]
fn web_server_goal_detail_exposes_failed_feature_blocking_notice() {
    let temp_root = unique_temp_dir("http-goal-feature-blocking-notice");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
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

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });

    assert_eq!(show.status, 200);
    assert_eq!(
        show.body["goal"]["feature_blocking_notice"]["feature_id"],
        "FEA1"
    );
    assert_eq!(
        show.body["goal"]["feature_blocking_notice"]["blocked_goal_ids"],
        json!(["GOAL2"])
    );
    assert!(
        show.body["goal"]["feature_blocking_notice"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("Submit a recovery round")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_updates_feature_metadata_and_runs_goal_actions() {
    let temp_root = unique_temp_dir("http-feature-goal-actions");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [
        ("GOAL1", "Verify Goal"),
        ("GOAL2", "Retry Quality"),
        ("GOAL3", "Retry Merge"),
        ("GOAL4", "Submit Merge"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Original Feature"})),
    });

    let feature = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/features/FEA1".to_string(),
        body: Some(json!({
            "name": "Renamed Feature",
            "description": "Updated description",
            "reporter": "QA"
        })),
    });
    assert_eq!(feature.status, 200);
    assert_eq!(feature.body["feature"]["name"], "Renamed Feature");
    assert_eq!(
        feature.body["feature"]["description"],
        "Updated description"
    );

    let goal_actions = FileWorkItemService::new(&refine_dir);
    goal_actions
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    for status in [
        GoalStatus::Plan,
        GoalStatus::Implement,
        GoalStatus::Quality,
        GoalStatus::Governance,
    ] {
        goal_actions
            .advance_automated_goal_status("GOAL1", status)
            .unwrap();
    }
    let verified = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/verify".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(verified.status, 400);
    assert_eq!(
        goal_actions.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Governance
    );

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL2", "GOAL3"],
            "update": {"status": "failed"}
        })),
    });
    let retry_quality = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL2/retry-quality".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(retry_quality.status, 200);
    assert_eq!(retry_quality.body["goal"]["status"], "quality");

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL4/start".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(started.status, 200);
    assert_eq!(started.body["goal"]["status"], "todo");
    FileWorkItemService::new(&refine_dir)
        .advance_automated_goal_status("GOAL4", GoalStatus::Plan)
        .unwrap();
    let submitted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL4/submit-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(submitted.status, 409);
    assert!(
        submitted.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("workflow-owned")
    );
    FileWorkItemService::new(&refine_dir)
        .advance_automated_goal_status("GOAL4", GoalStatus::Implement)
        .unwrap();
    FileWorkItemService::new(&refine_dir)
        .advance_automated_goal_status("GOAL4", GoalStatus::Quality)
        .unwrap();
    FileWorkItemService::new(&refine_dir)
        .advance_automated_goal_status("GOAL4", GoalStatus::Governance)
        .unwrap();
    let submitted_again = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL4/submit-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(submitted_again.status, 200);
    assert_eq!(submitted_again.body["goal"]["status"], "governance");

    let retry_merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL3/retry-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(retry_merge.status, 200);
    assert_eq!(retry_merge.body["goal"]["status"], "governance");

    let merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL3/merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(merge.status, 503);
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL3")
            .unwrap()
            .goal
            .status,
        GoalStatus::Governance
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_transfers_feature_ownership_as_a_unit() {
    let temp_root = unique_temp_dir("http-feature-node-transfer");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root);

    let create_node = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        body: Some(json!({"id": "remote-node", "display_name": "Remote Node"})),
    });
    assert_eq!(create_node.status, 200);
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Transfer Feature"})),
    });
    assert_eq!(create_feature.status, 201);
    for (id, name) in [("GOAL1", "Feature One"), ("GOAL2", "Feature Two")] {
        let goal = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
        assert_eq!(goal.status, 201);
        let assign = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/features/FEA1/goals/{id}"),
            body: None,
        });
        assert_eq!(assign.status, 200);
    }

    let direct_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-goals".to_string(),
        body: Some(json!({
            "item_id": "GOAL1",
            "target_node_id": "remote-node"
        })),
    });
    assert_eq!(direct_goal.status, 409);
    assert!(
        direct_goal.body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("transfer the Feature instead"),
        "{direct_goal:#?}"
    );

    let bulk_transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-features".to_string(),
        body: Some(json!({
            "selected_ids": ["FEA1"],
            "target_node_id": "remote-node"
        })),
    });
    assert_eq!(bulk_transfer.status, 200);
    assert_eq!(bulk_transfer.body["updated"], 3);
    assert_eq!(bulk_transfer.body["ids"], json!(["FEA1", "GOAL1", "GOAL2"]));

    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/transfer".to_string(),
        body: Some(json!({"target_node_id": "remote-node"})),
    });
    assert_eq!(transfer.status, 200);
    assert_eq!(transfer.body["updated"], 3);
    assert_eq!(transfer.body["ids"], json!(["FEA1", "GOAL1", "GOAL2"]));
    let feature = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(feature.body["feature"]["node_id"], "remote-node");
    for id in ["GOAL1", "GOAL2"] {
        let goal = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/goals/{id}"),
            body: None,
        });
        assert_eq!(goal.body["goal"]["node_id"], "remote-node");
    }

    remove_temp_dir(&temp_root);
}
