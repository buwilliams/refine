use super::*;

#[test]
fn web_server_serves_mcp_surface_through_daemon() {
    let server = server_with_projection();

    // The MCP surface is mounted by the always-on daemon web server, so a
    // JSON-RPC tools/call reaches a real capability route without any extra
    // process or transport.
    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/mcp".to_string(),
        body: Some(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "refine_list_goals", "arguments": {}},
        })),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["result"]["isError"], false);
    let goals = &response.body["result"]["structuredContent"]["goals"];
    assert!(goals.as_array().is_some());

    // GET reports server identity so clients can discover the surface.
    let identity = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/mcp".to_string(),
        body: None,
    });
    assert_eq!(identity.status, 200);
    assert_eq!(identity.body["serverInfo"]["name"], "refine");
}

#[test]
fn source_update_status_integration_drives_browser_states_across_reconnect() {
    use crate::tools::host::source_promotion::{
        SOURCE_PROMOTION_STATE_FILE, SourcePromotionOperation,
    };

    let temp_root = unique_temp_dir("source-update-status-integration");
    let runtime_root = temp_root.join("run/8080");
    let (seed, target_root) = seeded_remote_clone(&temp_root);
    fs::create_dir_all(target_root.join("src")).unwrap();
    fs::create_dir_all(target_root.join("scripts")).unwrap();
    fs::write(
        target_root.join("Cargo.toml"),
        "[package]\nname = \"refine\"\n",
    )
    .unwrap();
    fs::write(target_root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(target_root.join("scripts/install.sh"), "#!/bin/sh\n").unwrap();
    fs::write(target_root.join("r"), "#!/bin/sh\n").unwrap();
    git(&target_root, &["add", "."]).unwrap();
    git(
        &target_root,
        &["commit", "-m", "add Refine source entrypoints"],
    )
    .unwrap();
    git(&target_root, &["push", "origin", "main"]).unwrap();
    git(&seed, &["pull", "--ff-only"]).unwrap();
    fs::write(seed.join("remote.txt"), "new source\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "new source commit"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor.set_workflow_paused(true).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(target_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let available = server.handle_source_status_for_checkout(true, target_root.clone());
    assert_eq!(available.status, 200);
    assert_eq!(available.body["target_app_is_refine"], true);
    assert_eq!(available.body["source_update"]["visible"], true);
    assert_eq!(available.body["source_update"]["enabled"], true);
    assert_eq!(available.body["source_update"]["state"], "available");
    let current_commit = available.body["source"]["current_commit"]
        .as_str()
        .unwrap()
        .to_string();
    let available_commit = available.body["source"]["available_commit"]
        .as_str()
        .unwrap()
        .to_string();

    fs::write(target_root.join("dirty.txt"), "leave untouched\n").unwrap();
    let blocked = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(blocked.status, 200);
    assert_eq!(blocked.body["source_update"]["visible"], true);
    assert_eq!(blocked.body["source_update"]["enabled"], false);
    assert_eq!(blocked.body["source_update"]["state"], "blocked");
    fs::remove_file(target_root.join("dirty.txt")).unwrap();

    let mut operation = SourcePromotionOperation {
        id: "source-test".to_string(),
        status: "running".to_string(),
        stage: "restart_daemon".to_string(),
        message: "Source activated; restarting Refine".to_string(),
        checkout_path: target_root.display().to_string(),
        from_commit: current_commit,
        to_commit: available_commit,
        started_at: "2026-07-21T00:00:00Z".to_string(),
        updated_at: "2026-07-21T00:00:01Z".to_string(),
        error: None,
        rollback_attempted: false,
        rollback_succeeded: None,
        recovery: None,
    };
    fs::write(
        runtime_root.join(SOURCE_PROMOTION_STATE_FILE),
        serde_json::to_vec_pretty(&operation).unwrap(),
    )
    .unwrap();
    let reconnecting = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(reconnecting.status, 200);
    assert_eq!(reconnecting.body["source_update"]["enabled"], false);
    assert_eq!(reconnecting.body["source_update"]["state"], "updating");
    assert_eq!(
        reconnecting.body["source_update"]["title"],
        "Source activated; restarting Refine"
    );

    operation.status = "failed".to_string();
    operation.stage = "restart_daemon".to_string();
    operation.message = "Source promotion failed during restart_daemon".to_string();
    operation.error = Some("restart failed".to_string());
    operation.recovery = Some("Refine was restored; inspect and retry".to_string());
    fs::write(
        runtime_root.join(SOURCE_PROMOTION_STATE_FILE),
        serde_json::to_vec_pretty(&operation).unwrap(),
    )
    .unwrap();
    let failed = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(failed.status, 200);
    assert_eq!(failed.body["source"]["operation"]["status"], "failed");
    assert_eq!(failed.body["source_update"]["enabled"], true);
    assert_eq!(failed.body["source_update"]["state"], "available");

    server.target_root = Some(temp_root.join("not-refine"));
    let hidden = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(hidden.status, 200);
    assert_eq!(hidden.body["target_app_is_refine"], false);
    assert_eq!(hidden.body["source_update"]["visible"], false);
    assert_eq!(hidden.body["source_update"]["enabled"], false);
    assert_eq!(hidden.body["source_update"]["state"], "hidden");

    remove_temp_dir(&temp_root);
}

#[test]
fn source_promotion_api_preserves_unconfirmed_request_compatibility() {
    let server = server_with_projection();
    for body in [
        None,
        Some(json!({})),
        Some(json!({"confirmed": false})),
        Some(json!({"confirmed": true})),
    ] {
        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/system/source/promote".to_string(),
            body,
        });
        assert_eq!(response.status, 503);
        assert_eq!(response.body["error"]["code"], "runtime_root_unavailable");
    }
}

#[test]
fn release_api_previews_semver_and_rejects_unconfirmed_publication() {
    let runtime_root = unique_temp_dir("http-releases");
    fs::create_dir_all(&runtime_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let plan = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/releases/plan".to_string(),
        body: Some(json!({"bump": "patch"})),
    });
    assert_eq!(plan.status, 200, "{}", plan.body);
    assert_eq!(plan.body["plan"]["current_version"], "4.1.0");
    assert_eq!(plan.body["plan"]["proposed_version"], "4.1.1");
    assert_eq!(plan.body["plan"]["proposed_tag"], "4.1.1");

    let publish = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/releases/publish".to_string(),
        body: Some(json!({
            "confirmed": false,
            "preparation_id": "browser-controlled-value"
        })),
    });
    assert_eq!(publish.status, 400);
    assert!(
        publish.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("confirmed=true")
    );
    assert!(!releases_request_body_accepts_candidate_objects());

    let tampered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/releases/publish".to_string(),
        body: Some(json!({
            "confirmed": true,
            "preparation_id": "browser-controlled-value",
            "candidate": {
                "commit": "attacker-selected-commit",
                "worktree": "/tmp/attacker-selected-worktree"
            }
        })),
    });
    assert_eq!(tampered.status, 404, "{}", tampered.body);
    assert!(
        tampered.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("release request browser-controlled-value was not found")
    );

    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn web_server_cleans_activity_and_reports_unconnected_native_actions() {
    let temp_root = unique_temp_dir("http-cleanups");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/ui-error".to_string(),
        body: Some(json!({"message": "Boom"})),
    });
    assert!(refine_dir.join("logs/activity.jsonl").exists());
    let cleanup = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/cleanup".to_string(),
        body: Some(json!({"days": 0})),
    });
    assert_eq!(cleanup.status, 200);
    assert_eq!(cleanup.body["deleted"], 1);
    assert!(!refine_dir.join("logs/activity.jsonl").exists());

    let runtime_root = temp_root.join("run/8080");
    server.runtime_root = Some(runtime_root.clone());
    let metrics = FileMetricsService::new(&runtime_root);
    metrics
        .record_operation("old", 10.0, true, json!({}))
        .unwrap();
    let performance_before_cleanup = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/performance".to_string(),
        body: None,
    });
    assert_eq!(performance_before_cleanup.status, 200);
    assert_eq!(performance_before_cleanup.body["total_event_count"], 1);
    let performance_cleanup = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/performance/cleanup".to_string(),
        body: Some(json!({"clear": true})),
    });
    assert_eq!(performance_cleanup.status, 200);
    assert_eq!(performance_cleanup.body["deleted"], 1);
    assert!(!metrics.path().exists());
    let performance_after_cleanup = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/performance".to_string(),
        body: None,
    });
    assert_eq!(performance_after_cleanup.status, 200);
    assert_eq!(performance_after_cleanup.body["total_event_count"], 0);

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/changes/undo".to_string(),
        body: Some(json!({"commit": "abc123"})),
    });
    assert_eq!(undo.status, 202);
    let undo_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        undo.body["operation"]["id"].as_str().unwrap(),
        OperationState::Succeeded,
    );
    assert_eq!(undo_operation.result["ok"], false);

    let reset = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
        body: None,
    });
    assert_eq!(reset.status, 202);
    let reset_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        reset.body["operation"]["id"].as_str().unwrap(),
        OperationState::Succeeded,
    );
    assert_eq!(reset_operation.result["ok"], false);

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_rejects_retired_supervisor_routes() {
    let temp_root = unique_temp_dir("retired-supervisor-routes");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(temp_root.join("run/8082"));
    for (method, path) in [
        ("GET", "/api/supervisor-agent"),
        ("POST", "/api/supervisor-agent/session"),
    ] {
        let response = server.handle(ApiRequest {
            method: method.to_string(),
            path: path.to_string(),
            body: Some(json!({})),
        });
        assert_eq!(response.status, 404);
    }
    let terminal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"profile": "supervisor"})),
    });
    assert_eq!(terminal.status, 400);
    assert_eq!(terminal.body["error"]["code"], "invalid_input");
    remove_temp_dir(&temp_root);
}
