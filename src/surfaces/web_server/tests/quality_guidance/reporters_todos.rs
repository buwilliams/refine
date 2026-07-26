use super::*;

#[test]
fn web_server_manages_reporter_scoped_todo_lists_and_items() {
    let temp_root = unique_temp_dir("http-todos");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/todos/lists".to_string(),
        body: Some(json!({"reporter": "Buddy", "name": "Release"})),
    });
    assert_eq!(created.status, 201);
    let list_id = created.body["list"]["id"].as_str().unwrap().to_string();
    assert_eq!(created.body["list"]["reporter"], "Buddy");

    let added = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/todos/lists/{list_id}/items"),
        body: Some(json!({"reporter": "Buddy", "text": "Verify candidate"})),
    });
    assert_eq!(added.status, 201);
    let item_id = added.body["item"]["id"].as_str().unwrap().to_string();

    let completed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/todos/lists/{list_id}/items/{item_id}"),
        body: Some(json!({"reporter": "Buddy", "done": true})),
    });
    assert_eq!(completed.status, 200);
    assert_eq!(completed.body["item"]["done"], true);

    let undone_and_edited = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/todos/lists/{list_id}/items/{item_id}"),
        body: Some(json!({
            "reporter": "Buddy",
            "text": "Verify exact results",
            "done": false
        })),
    });
    assert_eq!(undone_and_edited.status, 200);
    assert_eq!(
        undone_and_edited.body["item"]["text"],
        "Verify exact results"
    );
    assert_eq!(undone_and_edited.body["item"]["done"], false);

    let other_reporter = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/todos?reporter=Alex".to_string(),
        body: None,
    });
    assert_eq!(other_reporter.status, 200);
    assert!(other_reporter.body["lists"].as_array().unwrap().is_empty());

    let wrong_reporter = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/todos/lists/{list_id}"),
        body: Some(json!({"reporter": "Alex", "name": "Not Alex's list"})),
    });
    assert_eq!(wrong_reporter.status, 404);

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/todos/lists/{list_id}"),
        body: Some(json!({"reporter": "Buddy", "name": "Ready for review"})),
    });
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.body["list"]["name"], "Ready for review");

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/todos?reporter=Buddy".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["lists"].as_array().unwrap().len(), 1);
    assert_eq!(
        listed.body["lists"][0]["items"].as_array().unwrap().len(),
        1
    );
    assert!(refine_dir.join("todo-lists.json").exists());

    let deleted_item = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: format!("/api/todos/lists/{list_id}/items/{item_id}"),
        body: Some(json!({"reporter": "Buddy"})),
    });
    assert_eq!(deleted_item.status, 200);
    assert!(
        deleted_item.body["lists"][0]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let deleted_list = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: format!("/api/todos/lists/{list_id}"),
        body: Some(json!({"reporter": "Buddy"})),
    });
    assert_eq!(deleted_list.status, 200);
    assert!(deleted_list.body["lists"].as_array().unwrap().is_empty());

    remove_temp_dir(&temp_root);
}
