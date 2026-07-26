use super::*;

#[test]
fn web_server_retries_workflow_executions() {
    let temp_root = unique_temp_dir("http-workflow-execution-retry");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let automation = WorkflowEngine::new(&runtime_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    let execution_id = automation.start_claim(&claim_id).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let retry = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/workflow/executions/{execution_id}/retry"),
        body: None,
    });
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body["retried_from"], execution_id);
    assert_eq!(retry.body["execution"]["goal_id"], "GOAL1");
    assert_eq!(retry.body["execution"]["status"], "running");
    assert_ne!(retry.body["execution"]["id"], execution_id);

    server.current_projection_with_runtime().unwrap();
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert!(cached.runtime.background_operations.is_empty());

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_resolves_app_scoped_routes_from_active_runtime_app() {
    let temp_root = unique_temp_dir("http-detached-active-app");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = None;
    server.runtime_root = Some(runtime_root.clone());

    let detached_settings = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/settings".to_string(),
        body: None,
    });
    assert_eq!(detached_settings.status, 503);
    assert_eq!(
        detached_settings.body["error"]["code"],
        "target_root_unavailable"
    );

    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(attached.status, 200);
    assert_eq!(attached.body["target_root"], app_root.display().to_string());
    assert!(runtime_root.join("apps.json").exists());
    assert!(!temp_root.join("run/apps.json").exists());

    let settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({"agent_cli": "smoke-ai"})),
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "smoke-ai");
    assert!(refine_dir.join("nodes.json").exists());
    assert!(!refine_dir.join("settings.json").exists());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"name": "Detached attach goal", "id": "GOAL1"})),
    });
    assert_eq!(created.status, 201);
    assert!(refine_dir.join("goals/GO/AL1/goal.json").exists());

    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let sse = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/api/sse".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(sse.status, 200);
    let sse_body = String::from_utf8(sse.body).unwrap();
    assert!(sse_body.contains("event: project_updated"));
    assert!(sse_body.contains("\"goal_count\":1"));

    remove_temp_dir(&temp_root);
}
