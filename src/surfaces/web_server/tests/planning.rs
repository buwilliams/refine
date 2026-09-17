use super::*;

#[test]
fn planning_api_retains_requests_revisions_and_goal_identity() {
    let temp = unique_temp_dir("http-planning");
    let mut server = server_with_projection();
    server.target_root = Some(temp.clone());
    server.runtime_root = Some(temp.join("runtime"));
    let request = |method: &str, path: &str, body| {
        server.handle(ApiRequest {
            method: method.into(),
            path: path.into(),
            body,
        })
    };
    let board_command =
        json!({"operation":"board.create","request_id":"board-request","data":{"name":"Shared"}});
    let response = request(
        "POST",
        "/api/planning/commands",
        Some(board_command.clone()),
    );
    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(response.body["state"], "complete");
    let board = response.body["result"].clone();
    assert_eq!(
        request("POST", "/planning/commands", Some(board_command)).body,
        response.body
    );
    let create = json!({"operation":"card.create","request_id":"card-request","board_id":board["id"],"lane_id":board["lanes"][0]["id"],"data":{"name":"Read a paper","reporter":"Buddy","description":"Shared notes"}});
    assert_eq!(
        request("POST", "/api/planning/commands", Some(create)).status,
        202
    );
    let root = refine_dir_for_target_root(&temp).unwrap();
    let planning =
        crate::application::planning::FilePlanningService::new(&root, &temp, temp.join("runtime"))
            .unwrap();
    planning.process_pending().unwrap();
    let snapshot = request("GET", "/api/planning", None);
    assert_eq!(snapshot.status, 200);
    let card = &snapshot.body["cards"][0];
    assert_eq!(card["goal"]["status"], "draft");
    assert_eq!(card["goal"]["round_count"], 0);
    let id = card["placement"]["goal_id"].as_str().unwrap();
    let before = card["goal"]["workflow_revision"].clone();
    let command = json!({"operation":"card.move","request_id":"move-request","goal_id":id,"board_id":board["id"],"lane_id":board["lanes"][1]["id"],"expected_revision":1});
    assert_eq!(
        request("POST", "/planning/commands", Some(command)).status,
        202
    );
    planning.process_pending().unwrap();
    let goal = FileWorkItemService::new(&root)
        .show_goal_detail(id)
        .unwrap();
    assert_eq!(goal["workflow_revision"], before);
    assert_eq!(goal["status"], "draft");
    assert_eq!(
        request("GET", "/planning/actions/move-request", None).body["state"],
        "complete"
    );
    let mut stale = json!({"operation":"card.move","request_id":"stale-request","goal_id":id,"board_id":board["id"],"lane_id":board["lanes"][0]["id"],"expected_revision":1});
    assert_eq!(
        request("POST", "/planning/commands", Some(stale.clone())).status,
        202
    );
    stale["data"] = json!({"position":5});
    assert_eq!(
        request("POST", "/planning/commands", Some(stale)).status,
        409
    );
    planning.process_pending().unwrap();
    assert_eq!(
        request("GET", "/planning/actions/stale-request", None).body["state"],
        "failed"
    );
    let migrated = request(
        "POST",
        "/planning/commands",
        Some(json!({"operation":"migrate","request_id":"migration-request"})),
    );
    assert_eq!(migrated.body["state"], "complete");
    assert_eq!(request("GET", "/api/todos", None).status, 410);
    let _ = fs::remove_dir_all(temp);
}

#[test]
fn draft_goal_api_preserves_feature_and_reporter_without_creating_a_round() {
    let temp = unique_temp_dir("http-draft-goal");
    let root = temp.join(".refine");
    let service = FileWorkItemService::new(&root);
    service
        .create_feature_summary("Research", Some("FEATURE1"), None, None, None)
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp.clone());
    let result = server.handle(ApiRequest { method:"POST".into(), path:"/api/goals".into(), body:Some(json!({"status":"draft","name":"Research idea","description":"Explore options","reporter":"Alice","feature_id":"FEATURE1"})) });
    assert_eq!(result.status, 201, "{}", result.body);
    let goal = service
        .show_goal_detail(result.body["goal"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(goal["feature_id"], "FEATURE1");
    assert_eq!(goal["reporter"], "Alice");
    assert_eq!(goal["description"], "Explore options");
    assert_eq!(goal["status"], "draft");
    assert_eq!(goal["round_count"], 0);
    let _ = fs::remove_dir_all(temp);
}
