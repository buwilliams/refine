use super::*;

#[test]
fn web_server_manages_fleet_operations_over_nodes() {
    let temp_root = unique_temp_dir("http-fleet-registry");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/fleet/nodes".to_string(),
        body: Some(json!({
            "id": "node-1",
            "display_name": "Node One",
            "ssh_host": "example.com",
            "ssh_user": "deploy",
            "ssh_identity_path": "~/.ssh/refine_ed25519",
            "target_app_path": "/srv/app"
        })),
    });
    assert_eq!(registered.status, 200);
    assert_eq!(registered.body["enabled"], true);
    let registered_node = registered.body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(registered_node["ssh_host"], "example.com");
    assert_eq!(registered_node["ssh_user"], "deploy");
    assert_eq!(
        registered_node["ssh_identity_path"],
        "~/.ssh/refine_ed25519"
    );
    assert!(!refine_dir.join("cluster.json").exists());

    let disabled = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/fleet/nodes/node-1".to_string(),
        body: Some(json!({"enabled": false, "ssh_port": 2222})),
    });
    assert_eq!(disabled.status, 200);
    let disabled_node = disabled.body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(disabled_node["enabled"], false);
    assert_eq!(disabled_node["ssh_port"], 2222);

    let bootstrap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/fleet/nodes/node-1/bootstrap".to_string(),
        body: Some(json!({"dry_run": true})),
    });
    assert_eq!(bootstrap.status, 200);
    assert_eq!(bootstrap.body["ok"], true);
    assert_eq!(bootstrap.body["dry_run"], true);
    assert!(
        bootstrap.body["result"]["command"]
            .as_str()
            .unwrap()
            .contains("ssh -p 2222")
    );
    assert!(
        bootstrap.body["result"]["command"]
            .as_str()
            .unwrap()
            .contains("-i '~/.ssh/refine_ed25519'")
    );
    assert!(
        bootstrap.body["result"]["command"]
            .as_str()
            .unwrap()
            .contains("'deploy@example.com'")
    );
    assert_eq!(
        bootstrap.body["fleet"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "node-1")
            .unwrap()["health"]["status"],
        "ready"
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_reports_dashboard_diagnostics_target_app_nodes_and_fleet() {
    let temp_root = unique_temp_dir("http-status-surfaces");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        temp_root.join("package.json"),
        r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest run"}}"#,
    )
    .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "target_app_start_command": "npm run dev",
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Dashboard Goal"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL2", "name": "Finished Dashboard Goal"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL3", "name": "Cancelled Dashboard Goal"})),
    });
    let create_node = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        body: Some(json!({"id": "refine2"})),
    });
    assert_eq!(create_node.status, 200);
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/rounds".to_string(),
        body: Some(json!({"reporter": "Alice", "prompt": "Works"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL2/rounds".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "assignee": "Carol",
            "prompt": "Works"
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL3/rounds".to_string(),
        body: Some(json!({"reporter": "Bob", "prompt": "Works"})),
    });
    // This dashboard fixture needs a historical terminal Goal. Product surfaces may only reach
    // Done through reviewed integration approval, which is covered by the merger tests.
    let done_goal_path = refine_dir.join("goals/GO/AL2/goal.json");
    let mut done_goal: serde_json::Value =
        serde_json::from_slice(&fs::read(&done_goal_path).unwrap()).unwrap();
    done_goal["status"] = json!("done");
    fs::write(
        &done_goal_path,
        format!("{}\n", serde_json::to_string_pretty(&done_goal).unwrap()),
    )
    .unwrap();
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL3"],
            "update": {"status": "cancelled"}
        })),
    });
    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-goals".to_string(),
        body: Some(json!({
            "target_node_id": "refine2",
            "selected_ids": ["GOAL3"],
            "filter": {}
        })),
    });
    assert_eq!(transfer.status, 200);
    FileActivityService::new(&refine_dir)
        .append(ActivityEntry {
            id: "act-dashboard".to_string(),
            datetime: "2026-06-05T00:00:00Z".to_string(),
            severity: "info".to_string(),
            category: "state".to_string(),
            message: "Dashboard activity".to_string(),
            goal_id: Some("GOAL1".to_string()),
            actor: Some("system".to_string()),
            details: None,
            actions: Vec::new(),
        })
        .unwrap();
    let rebuilt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cache/rebuild".to_string(),
        body: None,
    });
    assert_eq!(rebuilt.status, 200);

    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=current".to_string(),
        body: None,
    });
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body["node_filter"], "current");
    assert_eq!(dashboard.body["counts"]["backlog"], 1);
    assert_eq!(
        dashboard.body["counts"]["cancelled"],
        serde_json::Value::Null
    );
    assert_eq!(dashboard.body["active_node_id"], "default");
    assert_eq!(dashboard.body["activity"][0]["id"], "act-dashboard");
    let all_dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_dashboard.status, 200);
    assert_eq!(all_dashboard.body["node_filter"], "all");
    assert_eq!(all_dashboard.body["counts"]["backlog"], 1);
    assert_eq!(all_dashboard.body["counts"]["cancelled"], 1);
    assert_eq!(
        all_dashboard.body["counts"],
        all_dashboard.body["all_node_counts"]
    );
    let assignee_stats = dashboard.body["assignee_stats"].as_array().unwrap();
    let alice = assignee_stats
        .iter()
        .find(|row| row["assignee"] == "Alice")
        .unwrap();
    assert_eq!(alice["assigned"], 1);
    assert_eq!(alice["active"], 1);
    assert_eq!(alice["done"], 0);
    assert_eq!(alice["completion_rate"], 0.0);
    let carol = assignee_stats
        .iter()
        .find(|row| row["assignee"] == "Carol")
        .unwrap();
    assert_eq!(carol["assigned"], 1);
    assert_eq!(carol["active"], 0);
    assert_eq!(carol["done"], 1);
    assert_eq!(carol["completion_rate"], 100.0);
    let cached = FileProjectProjectionStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.runtime.target_app.unwrap()["app_url"],
        "http://127.0.0.1:3000"
    );
    assert_eq!(
        cached.runtime.preflight.unwrap()["stage"],
        "provider_detection"
    );

    let diagnostics = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/diagnostics".to_string(),
        body: None,
    });
    assert_eq!(diagnostics.status, 200);
    assert_eq!(diagnostics.body["reachable"], true);
    for key in [
        "daemon",
        "install",
        "os_backend",
        "target_app",
        "git",
        "provider",
        "browser",
        "docker",
        "storage",
    ] {
        assert!(
            diagnostics.body["doctor"][key]
                .as_array()
                .map(|items| !items.is_empty())
                .unwrap_or(false),
            "missing doctor section {key}"
        );
    }
    assert!(
        diagnostics.body["doctor"]["target_app"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry
                .as_str()
                .unwrap_or("")
                .contains("supervised by the native daemon"))
    );
    assert!(
        diagnostics.body["doctor"]["storage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry
                .as_str()
                .unwrap_or("")
                .contains("runtime_root_exists="))
    );

    let target = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/target-app/status".to_string(),
        body: None,
    });
    assert_eq!(target.status, 200);
    assert_eq!(target.body["app_url"], "http://127.0.0.1:3000");
    assert_eq!(target.body["has_start_command"], true);

    let generated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/generate-instructions".to_string(),
        body: Some(json!({"kind": "all", "provider": "__local__"})),
    });
    assert_eq!(generated.status, 200);
    assert!(
        generated.body["config"]["start_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run dev")
    );
    assert_eq!(generated.body["config"]["start_command"], "");
    assert!(
        generated.body["settings"]["target_app_build_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run build")
    );
    assert_eq!(
        generated.body["settings"]["target_app_test_command"],
        "npm test"
    );
    assert_eq!(
        generated.body["settings"]["target_app_test_commands"],
        r#"[{"command":"npm test","enabled":true}]"#
    );
    assert_eq!(generated.body["config"]["tcp_check_port"], "3000");
    assert!(!temp_root.join(".refine/manage-app.sh").exists());

    let generated_operation = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/generate-instructions".to_string(),
        body: Some(json!({"kind": "all", "provider": "__local__", "background": true})),
    });
    assert_eq!(
        generated_operation.status, 202,
        "{:?}",
        generated_operation.body
    );
    let generated_operation_id = generated_operation.body["operation"]["id"]
        .as_str()
        .unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let generated_operation =
        wait_for_operation_status(&registry, generated_operation_id, OperationState::Succeeded);
    assert!(
        generated_operation.result["config"]["start_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run dev")
    );
    assert_eq!(generated_operation.result["config"]["start_command"], "");
    let settings = FileSettingsService::new(&refine_dir).load().unwrap();
    assert!(
        settings["target_app_start_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run dev")
    );
    assert_eq!(settings["target_app_test_command"], "npm test");
    assert_eq!(
        settings["target_app_test_commands"],
        r#"[{"command":"npm test","enabled":true}]"#
    );

    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_build_instructions": "",
            "target_app_build_command": ""
        }))
        .unwrap();
    let rebuild = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/target-app-builder/build".to_string(),
        body: None,
    });
    assert_eq!(rebuild.status, 202);
    let rebuild_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        rebuild.body["operation"]["id"].as_str().unwrap(),
        OperationState::Succeeded,
    );
    assert_eq!(rebuild_operation.result["queued"], false);

    let nodes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/nodes".to_string(),
        body: None,
    });
    assert_eq!(nodes.status, 200);
    assert_eq!(nodes.body["nodes"][0]["id"], "default");

    let fleet = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/fleet".to_string(),
        body: None,
    });
    assert_eq!(fleet.status, 200);
    assert_eq!(fleet.body["enabled"], true);
    assert_eq!(fleet.body["nodes"][0]["id"], "default");

    remove_temp_dir(&temp_root);
}
