use super::*;

#[test]
fn web_server_creates_features_and_updates_membership() {
    let temp_root = unique_temp_dir("http-feature-membership");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Goal One"})),
    });

    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);
    assert_eq!(create_feature.body["feature"]["id"], "FEA1");

    let add_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals".to_string(),
        body: Some(json!({"goal_id": "GOAL1"})),
    });
    assert_eq!(add_goal.status, 200);
    assert_eq!(add_goal.body["goal_ids"], json!(["GOAL1"]));

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["goal_ids"], json!(["GOAL1"]));
    assert_eq!(show.body["feature"]["goal_ids"], json!(["GOAL1"]));
    assert_eq!(show.body["feature"]["goal_count"], 1);
    assert_eq!(show.body["feature"]["goals"][0]["id"], "GOAL1");

    let unorder_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/unorder".to_string(),
        body: None,
    });
    assert_eq!(unorder_goal.status, 200);
    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(
        show.body["feature"]["goals"][0]["feature_order"],
        json!(null)
    );

    let order_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/order".to_string(),
        body: None,
    });
    assert_eq!(order_goal.status, 200);
    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(show.body["feature"]["goals"][0]["feature_order"], json!(1));

    let remove_goal = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(remove_goal.status, 200);
    assert_eq!(remove_goal.body["goal_ids"], json!([]));

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_feature_goal_authoring_is_one_policy_driven_api_operation() {
    let temp_root = unique_temp_dir("http-feature-goal-authoring");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    assert_eq!(
        server
            .handle(ApiRequest {
                method: "POST".to_string(),
                path: "/api/features".to_string(),
                body: Some(json!({"id": "FEA1", "name": "Feature One"})),
            })
            .status,
        201
    );

    let first = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "name": "Foundation",
            "prompt": "Build the foundation",
            "reporter": "Buddy",
            "priority": "low",
            "placement": "first"
        })),
    });
    assert_eq!(first.status, 201, "{:#}", first.body);
    let first_id = first.body["goal"]["id"].as_str().unwrap().to_string();
    assert_eq!(first.body["goal"]["feature_order"], 1);

    let second = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "name": "Feature UI",
            "prompt": "Bind the inline composer",
            "reporter": "Buddy",
            "priority": "high",
            "placement": {"after": first_id}
        })),
    });
    assert_eq!(second.status, 201, "{:#}", second.body);
    let second_id = second.body["goal"]["id"].as_str().unwrap().to_string();
    assert_eq!(second.body["goal"]["feature_order"], 2);

    let duplicate = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "prompt": "Bind the inline composer",
            "reporter": "Buddy",
            "priority": "low",
            "placement": "unordered"
        })),
    });
    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.body["error"]["code"], "duplicate_goal");
    assert_eq!(
        duplicate.body["error"]["duplicate"]["match"]["id"],
        second_id
    );

    let goal_path = refine_dir
        .join("goals")
        .join(&second_id[..2])
        .join(&second_id[2..])
        .join("goal.json");
    let mut review_goal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    review_goal["status"] = json!("review");
    fs::write(&goal_path, serde_json::to_vec_pretty(&review_goal).unwrap()).unwrap();

    let shown = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features/FEA1".to_string(),
        body: None,
    });
    let review = shown.body["feature"]["goals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|goal| goal["id"] == second_id)
        .unwrap();
    assert_eq!(review["feature_authoring"]["editable"], true);

    let edited = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "goal_id": second_id,
            "name": "Reviewed UI",
            "prompt": "Revise while the Goal is in review",
            "reporter": "Buddy",
            "priority": "medium",
            "placement": "first"
        })),
    });
    assert_eq!(edited.status, 200, "{:#}", edited.body);
    assert_eq!(edited.body["goal"]["status"], "review");
    assert_eq!(edited.body["goal"]["feature_order"], 1);
    assert_eq!(edited.body["goal"]["name"], "Reviewed UI");

    let invalid = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "prompt": "Invalid placement",
            "reporter": "Buddy",
            "priority": "low",
            "placement": {"after": "MISSING"}
        })),
    });
    assert_eq!(invalid.status, 400);
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .list_goal_summaries()
            .unwrap()
            .len(),
        2
    );

    remove_temp_dir(&temp_root);
}
