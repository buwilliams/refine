use super::*;

#[test]
fn daemon_startup_recovers_quality_cancellation_for_original_app_after_switch() {
    let temp_root = unique_temp_dir("http-quality-cancellation-app-routing");
    let app_a = temp_root.join("app-a");
    let app_b = temp_root.join("app-b");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&app_a);
    init_git_app(&app_b);
    let refine_a = refine_dir_for_target_root(&app_a).unwrap();
    let refine_b = refine_dir_for_target_root(&app_b).unwrap();
    let apps = crate::tools::product::project_registry::FileProjectRegistryService::new(
        &runtime_root,
        None,
    );
    apps.register_path(Some("App A"), app_a.to_str().unwrap(), true)
        .unwrap();
    apps.register_path(Some("App B"), app_b.to_str().unwrap(), true)
        .unwrap();
    let work_items_a = FileWorkItemService::new(&refine_a);
    work_items_a
        .create_goal_summary("Recover on original app", Some("GOAL1"))
        .unwrap();
    work_items_a
        .append_goal_round_summary("GOAL1", "Buddy", "Cancel safely")
        .unwrap();
    let goal_summary = work_items_a.show_goal_summary("GOAL1").unwrap();
    let goal_path = refine_a.join(goal_summary.goal.json_path);
    let mut goal_value: serde_json::Value =
        serde_json::from_slice(&fs::read(&goal_path).unwrap()).unwrap();
    goal_value["node_id"] = json!("node-a");
    fs::write(&goal_path, serde_json::to_vec_pretty(&goal_value).unwrap()).unwrap();

    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry
        .register_exclusive_with_request(
            "quality:GOAL1:candidate-a",
            json!({
                "goal_id": "GOAL1",
                "round_idx": 0,
                "node_id": "node-a",
                "provider": "smoke-ai",
                "cwd": app_a.display().to_string(),
                "candidate_commit": "candidate-a",
                "target_root": app_a.display().to_string(),
                "refine_dir": refine_a.display().to_string(),
                "defer_cancellation_terminal": true,
                "test_inject_recovery_termination_failure": true
            }),
        )
        .unwrap();

    let managed = FileProcessSupervisor::new(&runtime_root)
        .launch(operation_helper_process_spec(&operation.id))
        .unwrap();
    registry.cancel(&operation.id).unwrap();
    let pid = managed.pid.unwrap();
    assert!(managed_pid_is_alive(pid).unwrap());

    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    daemon.recover_runtime_state().unwrap();

    let incomplete = registry.status(&operation.id).unwrap();
    assert_eq!(incomplete.state, OperationState::Cancelling);
    assert_eq!(
        incomplete.error.unwrap()["code"],
        "operation_recovery_process_termination_failed"
    );
    assert!(managed_pid_is_alive(pid).unwrap());
    let detail_a = work_items_a.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail_a["rounds"][0]["quality_state"], "unclassified",
        "terminal cancellation evidence must wait until every owned process exits"
    );
    assert!(
        FileWorkItemService::new(&refine_b)
            .show_goal_detail("GOAL1")
            .is_err(),
        "recovery must not write original-app evidence into the selected unrelated app"
    );

    let operation_path = runtime_root
        .join("operations")
        .join(format!("{}.json", operation.id));
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(&operation_path).unwrap()).unwrap();
    stored["request"]["test_inject_recovery_termination_failure"] = json!(false);
    fs::write(&operation_path, serde_json::to_vec_pretty(&stored).unwrap()).unwrap();

    daemon.recover_runtime_state().unwrap();
    wait_for_managed_pid_exit(pid);
    assert_eq!(
        registry.status(&operation.id).unwrap().state,
        OperationState::Cancelled
    );
    let detail_a = work_items_a.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail_a["rounds"][0]["quality_state"], "cancelled");
    daemon.recover_runtime_state().unwrap();
    let cancelled_logs = FileLogService::new(&refine_a)
        .all_round_logs("GOAL1")
        .unwrap()
        .into_iter()
        .filter(|entry| {
            entry.round_idx == Some(0)
                && entry.entry.message == "Quality checks cancelled."
                && entry
                    .entry
                    .details
                    .as_ref()
                    .and_then(|details| details.get("operation_id"))
                    == Some(&json!(operation.id))
        })
        .count();
    assert_eq!(cancelled_logs, 1, "replayed settlement must be idempotent");
    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_manages_quality_settings_and_checks() {
    let temp_root = unique_temp_dir("http-quality");
    let app_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    init_git_app(&app_root);
    git(&app_root, &["branch", "-m", "refine/GOAL1/round-1"]).unwrap();
    let candidate_commit = git_stdout(&app_root, &["rev-parse", "HEAD"]);
    let refine_dir = refine_dir_for_target_root(&app_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Quality candidate", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "test", "Verify candidate")
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &candidate_commit,
            Some(&candidate_commit),
        )
        .unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"summary\":\"Dashboard Quality planned.\",\"results\":[{\"test\":\"Dashboard loads for a signed-in user.\",\"status\":\"passed\",\"evidence\":\"Focused browser check planned\",\"command\":\"printf dashboard-ok\"}]}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai) };
    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let app_settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "agent_cli": "smoke-ai",
            "target_app_test_commands": [
                {"command": "printf target-test-ok", "enabled": true},
                {"command": "printf skipped", "enabled": false}
            ]
        })),
    });
    assert_eq!(app_settings.status, 200);
    assert_eq!(
        app_settings.body["settings"]["target_app_test_command"],
        "printf target-test-ok"
    );

    let initial = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        body: None,
    });
    assert_eq!(initial.status, 200);
    assert_eq!(initial.body["enabled"], "1");
    assert_eq!(initial.body["timing"], "pre_merge");
    assert_eq!(initial.body["tests"], json!([]));

    let saved = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/quality".to_string(),
        body: Some(json!({
            "enabled": "1",
            "timing": "post_build",
            "business_requirements": "Dashboard must render",
            "instructions": "Run focused checks",
            "tests": ["Dashboard loads for a signed-in user."]
        })),
    });
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["enabled"], "1");
    assert_eq!(saved.body["timing"], "post_build");
    assert_eq!(saved.body["configured"], true);

    let legacy_timing = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({"quality_timing": "pre_merge"})),
    });
    assert_eq!(legacy_timing.status, 200);
    assert_eq!(
        legacy_timing.body["settings"]["quality_timing"],
        "pre_merge"
    );
    let effective_quality = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        body: None,
    });
    assert_eq!(effective_quality.body["timing"], "pre_merge");
    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(dashboard.body["quality_timing"], "pre_merge");
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    assert!(
        nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["settings"].get("quality_timing").is_none())
    );

    let checks = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/checks".to_string(),
        body: Some(json!({"goal_id": "GOAL1"})),
    });
    assert_eq!(checks.status, 202);
    assert_eq!(checks.body["ok"], true);
    assert!(
        checks.body["operation"]["owner"]
            .as_str()
            .unwrap()
            .starts_with("quality:GOAL1:")
    );
    assert_eq!(checks.body["operation"]["status"], "running");
    let quality_operation_id = checks.body["operation"]["id"].as_str().unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = (0..200)
        .find_map(|_| {
            let operation = registry.status(quality_operation_id).unwrap();
            if matches!(
                operation.state,
                OperationState::Succeeded | OperationState::Failed
            ) {
                Some(operation)
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                None
            }
        })
        .expect("Quality operation did not settle");
    assert_eq!(operation.state, OperationState::Succeeded);
    assert_eq!(operation.result["owner_id"], "GOAL1");
    assert_eq!(
        operation.result["results"][0]["test"],
        "Dashboard loads for a signed-in user."
    );
    assert!(operation.result["results"][0]["process_id"].is_string());
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "passed");
    assert_eq!(
        detail["rounds"][0]["quality_details"]["results"],
        operation.result["results"]
    );
    let command_override = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/checks".to_string(),
        body: Some(json!({"command": "printf bypass"})),
    });
    assert_eq!(command_override.status, 400);
    assert!(
        command_override.body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("plain-text tests")
    );
    let quality_operation_logs = FileOperationRegistry::new(&runtime_root)
        .page_logs(quality_operation_id, 10, 0)
        .unwrap()
        .0;
    assert!(
        quality_operation_logs
            .iter()
            .any(|log| log.message == "Quality checks passed")
    );

    let screenshots = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality/screenshots?owner_id=GOAL1".to_string(),
        body: None,
    });
    assert_eq!(screenshots.status, 200);
    assert_eq!(screenshots.body["owner_id"], "GOAL1");
    assert_eq!(screenshots.body["screenshot_count"], 0);
    assert!(
        screenshots.body["screenshots"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert!(listed.body.get("regressions").is_none());

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    remove_temp_dir(&temp_root);
}
