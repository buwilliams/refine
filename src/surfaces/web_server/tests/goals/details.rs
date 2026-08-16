use super::*;

#[test]
fn web_server_routes_work_goal_queries_through_projection() {
    let mut server = server_with_projection();
    server.projection.goals.insert(
        "GOAL2".to_string(),
        GoalSummaryProjection {
            goal: GoalIndexProjection {
                id: "GOAL2".to_string(),
                name: "Settings route".to_string(),
                status: GoalStatus::Done,
                priority: GoalPriority::High,
                reporter: Some("Alice".to_string()),
                assignee: Some("Alice".to_string()),
                round_count: 3,
                created: "created2".to_string(),
                updated: "updated2".to_string(),
                branch_name: None,
                node_id: Some("node-b".to_string()),
                feature_id: Some("FEA1".to_string()),
                feature_order: Some(1),
                json_path: "goals/02/GOAL2/goal.json".to_string(),
            },
            node_display_name: Some("Node B".to_string()),
            latest_round_prompt: None,
            searchable_text: "Settings route Alice".to_string(),
            activity_ids: Vec::new(),
        },
    );
    server.projection.features.insert(
        "FEA1".to_string(),
        FeatureSummaryProjection {
            feature: FeatureIndexProjection {
                id: "FEA1".to_string(),
                name: "Settings Feature".to_string(),
                description: Some("Settings work".to_string()),
                reporter: Some("Alice".to_string()),
                assignee: Some("Alice".to_string()),
                node_id: Some("node-b".to_string()),
                created: "created".to_string(),
                updated: "updated".to_string(),
                json_path: "features/FE/A1/feature.json".to_string(),
            },
            status: GoalStatus::Done,
            goal_ids: vec!["GOAL2".to_string()],
            rollup: FeatureRollup {
                status: GoalStatus::Done,
                goal_count: 1,
                done_count: 1,
                active_count: 0,
                failed_count: 0,
                cancelled_count: 0,
                blocked_count: 0,
                next_goal: None,
            },
        },
    );
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals".to_string(),
        body: None,
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["goals"].as_array().unwrap().len(), 2);
    assert_eq!(response.body["counts"]["todo"], 1);
    assert_eq!(response.body["counts"]["done"], 1);

    let filtered = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?reporter=Alice&feature=FEA1&rounds_gte=2&sort=priority&dir=desc&limit=1"
            .to_string(),
        body: None,
    });
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.body["goals"][0]["id"], "GOAL2");
    assert_eq!(filtered.body["filtered_counts"]["done"], 1);
    assert_eq!(filtered.body["matching_ids"], json!(["GOAL2"]));
    assert_eq!(filtered.body["page"]["total"], 1);
    assert!(filtered.body.get("facets").is_none());

    let status_facets = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?status=todo&reporter=Alice&feature=FEA1&rounds_gte=2&facets=1"
            .to_string(),
        body: None,
    });
    assert_eq!(status_facets.status, 200);
    assert_eq!(status_facets.body["goals"].as_array().unwrap().len(), 0);
    assert_eq!(
        status_facets.body["filtered_counts"]
            .as_object()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(status_facets.body["facets"]["status_counts"]["done"], 1);

    let features = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features?q=settings&status=done&reporter=Alice&node=node-b".to_string(),
        body: None,
    });
    assert_eq!(features.status, 200);
    assert_eq!(features.body["features"][0]["feature"]["id"], "FEA1");
    assert_eq!(features.body["matching_ids"], json!(["FEA1"]));
}

#[test]
fn web_server_instantly_promotes_new_goal_when_configured() {
    let temp_root = unique_temp_dir("http-goal-create-instant-promote");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    FileSettingsService::with_active_root(&refine_dir, &runtime_root)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "id": "GOAL1",
            "name": "Instantly promoted Goal",
            "reporter": "Reporter",
            "prompt": "Implement the instantly promoted Goal"
        })),
    });

    assert_eq!(created.status, 201);
    assert_eq!(created.body["goal"]["status"], "todo");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_edits_notes_and_deletes_goal() {
    let temp_root = unique_temp_dir("http-edit-note-delete");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Original"})),
    });

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: Some(json!({"name": "Renamed", "priority": "high"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["goal"]["name"], "Renamed");
    assert_eq!(edit.body["goal"]["priority"], "high");

    let note = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals/GOAL1/notes".to_string(),
        body: Some(json!({"author": "Reviewer", "body": "Needs context"})),
    });
    assert_eq!(note.status, 200);
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"body\": \"Needs context\""));
    let written_goal = serde_json::from_str::<serde_json::Value>(&written).unwrap();
    let note_id = written_goal["notes"][0]["id"].as_str().unwrap();

    let edited_note = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: Some(json!({
            "notes": [{
                "id": note_id,
                "author": "Reviewer",
                "body": "Updated context",
                "created": written_goal["notes"][0]["created"].clone()
            }]
        })),
    });
    assert_eq!(edited_note.status, 200);
    let edited_detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(
        edited_detail.body["goal"]["notes"][0]["body"],
        "Updated context"
    );

    let deleted_note = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: Some(json!({"notes": []})),
    });
    assert_eq!(deleted_note.status, 200);
    let deleted_detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(deleted_detail.body["goal"]["notes"], json!([]));

    let delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(delete.status, 200);
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());

    remove_temp_dir(&temp_root);
}

#[test]
fn daemon_agent_automation_loop_executes_todo_goals_without_manual_request() {
    let temp_root = unique_temp_dir("daemon-agent-automation-loop");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    fs::write(temp_root.join(".git/info/exclude"), "smoke-ai\n").unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '\\n# automated by smoke-ai loop\\n' >> app.py\nprintf '%s\\n' 'smoke-ai loop response'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "target_app_build_command": "printf build-ok",
            "allowed_commands": "printf"
        }))
        .unwrap();

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "id": "GOAL1",
            "name": "Loop schedulable",
            "reporter": "Reporter",
            "prompt": "Implement the schedulable Goal"
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/transition".to_string(),
        body: Some(json!({"status": "todo"})),
    });

    let daemon = LocalHttpDaemon::new(server.clone(), None);
    let automation_loop = daemon.start_agent_automation_loop(Duration::from_millis(25));
    #[cfg(not(target_os = "macos"))]
    let automation_timeout = Duration::from_secs(15);
    // Parallel debug tests perform many durable APFS writes; keep the Linux
    // budget while allowing the same workflow to finish under macOS load.
    #[cfg(target_os = "macos")]
    let automation_timeout = Duration::from_secs(30);
    let deadline = Instant::now() + automation_timeout;
    loop {
        let show = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/goals/GOAL1".to_string(),
            body: None,
        });
        assert_eq!(show.status, 200);
        if show.body["goal"]["status"] == "review" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "automation loop did not execute GOAL1 before timeout: {}",
            show.body["goal"]["status"]
        );
        thread::sleep(Duration::from_millis(25));
    }
    automation_loop.stop_for_test();

    assert!(!runtime_root.join("workflow-automation-state.json").exists());
    assert!(
        !fs::read_to_string(runtime_root.join(API_EVENTS_FILE))
            .unwrap_or_default()
            .contains("/workflow/")
    );

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_manages_nodes_and_transfers_goal_ownership() {
    let temp_root = unique_temp_dir("http-node-transfer");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    for (id, name) in [
        ("GOAL1", "Transfer One"),
        ("GOAL2", "Transfer Two"),
        ("GOAL3", "Stay Default"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        body: Some(json!({"display_name": "Remote QA"})),
    });
    assert_eq!(created.status, 200);
    assert!(
        created.body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "remote-qa")
    );

    let activated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/activate".to_string(),
        body: Some(json!({"node_id": "remote-qa"})),
    });
    assert_eq!(activated.status, 200);
    assert_eq!(activated.body["active_node_id"], "remote-qa");
    assert!(refine_dir.join("runtime/active-node.json").exists());
    assert!(!refine_dir.join("active-node.json").exists());

    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-goals".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL2"],
            "target_node_id": "remote-qa"
        })),
    });
    assert_eq!(transfer.status, 200);
    assert_eq!(transfer.body["updated"], 2);
    let current_node_goals = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?node=current".to_string(),
        body: None,
    });
    assert_eq!(current_node_goals.status, 200);
    assert_eq!(current_node_goals.body["page"]["total"], 2);
    assert_eq!(
        current_node_goals.body["goals"][0]["node_display_name"],
        "Remote QA"
    );
    let all_node_goals = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_node_goals.status, 200);
    assert_eq!(all_node_goals.body["page"]["total"], 3);
    let current_dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(current_dashboard.status, 200);
    assert_eq!(current_dashboard.body["node_filter"], "current");
    assert_eq!(current_dashboard.body["active_node_id"], "remote-qa");
    assert_eq!(current_dashboard.body["counts"]["backlog"], 2);
    let all_dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_dashboard.status, 200);
    assert_eq!(all_dashboard.body["node_filter"], "all");
    assert_eq!(all_dashboard.body["counts"]["backlog"], 3);
    let goal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(goal.body["goal"]["node_id"], "remote-qa");

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/nodes/remote-qa".to_string(),
        body: Some(json!({"display_name": "Remote QA Renamed"})),
    });
    assert_eq!(renamed.status, 200);
    assert!(
        renamed.body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["display_name"] == "Remote QA Renamed")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn stale_default_label_never_relabels_default_owned_goal_projections() {
    let temp_root = unique_temp_dir("http-stale-default-node-label");
    let target_root = temp_root.join("app");
    let refine_dir = target_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let mut server = server_with_projection();
    server.target_root = Some(target_root);
    server.runtime_root = Some(runtime_root);
    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOALDEFAULT", "name": "Default-owned Goal"})),
    });
    assert_eq!(created.status, 201);
    fs::write(
        refine_dir.join(crate::tools::product::nodes::NODE_REGISTRY_FILE),
        serde_json::json!({
            "nodes": [{
                "id": "default",
                "display_name": "BO2LNXNEVO04 (QA)",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z"
            }]
        })
        .to_string(),
    )
    .unwrap();

    let nodes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/nodes".to_string(),
        body: None,
    });
    assert_eq!(nodes.status, 200);
    assert_eq!(nodes.body["active_node_id"], "default");
    assert_eq!(nodes.body["active_node"], "Default");
    assert_eq!(nodes.body["nodes"][0]["display_name"], "Default");
    assert_eq!(
        nodes.body["nodes"][0]["registry_display_name"],
        "BO2LNXNEVO04 (QA)"
    );
    assert_eq!(
        nodes.body["diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );

    let project_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/project/status".to_string(),
        body: None,
    });
    assert_eq!(project_status.status, 200);
    assert_eq!(project_status.body["active_node_id"], "default");
    assert_eq!(project_status.body["active_node"], "Default");
    assert_eq!(
        project_status.body["active_node_diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );

    let list = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?node=all".to_string(),
        body: None,
    });
    assert_eq!(list.status, 200);
    assert_eq!(list.body["goals"][0]["node_id"], "default");
    assert_eq!(list.body["goals"][0]["node_display_name"], "Default");
    assert_eq!(
        list.body["goals"][0]["node_identity_diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );

    let detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOALDEFAULT".to_string(),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(detail.body["goal"]["node_id"], "default");
    assert_eq!(detail.body["goal"]["node_display_name"], "Default");
    assert_eq!(
        detail.body["goal"]["node_identity_diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );

    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body["active_node_id"], "default");
    assert_eq!(dashboard.body["active_node_display_name"], "Default");
    assert_eq!(
        dashboard.body["active_node_diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );

    remove_temp_dir(&temp_root);
}
