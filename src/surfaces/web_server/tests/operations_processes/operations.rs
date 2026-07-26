use super::*;

#[test]
fn operation_cancel_route_is_a_thin_shared_capability_adapter() {
    let routes = include_str!("../../operation_routes/operations.rs");
    let handler = routes
        .split("pub(in crate::surfaces::web_server) fn handle_operation_cancel")
        .nth(1)
        .unwrap();

    assert!(handler.contains("registry.cancel_supervised"));
    assert!(!handler.contains("current_projection_with_runtime"));
    assert!(!handler.contains("request_termination"));
    assert!(!handler.contains("fail_with_error"));
    assert!(!routes.contains("fn terminate_operation_processes"));
}

#[test]
fn web_server_reads_and_cancels_runtime_operations() {
    let temp_root = unique_temp_dir("http-operations");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry.register("bulk_update_goals").unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{}", operation.id),
        body: None,
    });
    assert_eq!(status.status, 200, "{:#}", status.body);
    assert_eq!(status.body["operation"]["status"], "running");
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(cached.runtime.background_operations[0]["id"], operation.id);
    assert_eq!(cached.runtime.background_operations[0]["status"], "running");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{}/cancel", operation.id),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["operation"]["status"], "cancelled");
    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{}/logs?limit=10", operation.id),
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["total"], 2);
    assert!(
        logs.body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["message"] == "Operation cancelled")
    );
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.runtime.background_operations[0]["status"],
        "cancelled"
    );

    remove_temp_dir(&temp_root);
}
