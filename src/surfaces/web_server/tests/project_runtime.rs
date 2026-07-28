use super::*;

#[test]
fn web_server_structures_dashboard_attention_and_runtime_banner() {
    let mut server = server_with_projection();
    server
        .projection
        .goals
        .get_mut("GOAL1")
        .unwrap()
        .goal
        .status = GoalStatus::Failed;
    server.projection.runtime.supervisor = json!({"runner_reachable": false}).as_object().cloned();

    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(response.status, 200);
    assert_eq!(response.body["runner_reachable"], json!(false));
    let attention = response.body["needs_attention"].as_array().unwrap();
    assert!(attention.iter().any(|item| {
        item["kind"] == "filter"
            && item["message"] == "1 failed Goal(s) need recovery"
            && item["severity"] == "warn"
            && item["filter"] == json!({"status": "failed"})
    }));
    assert!(attention.iter().any(|item| {
        item["kind"] == "banner"
            && item["severity"] == "error"
            && item["message"]
                .as_str()
                .unwrap()
                .contains("Refine cannot reach the runtime worker")
    }));
}

#[test]
fn web_server_reports_project_registry_and_updates_settings() {
    let temp_root = unique_temp_dir("http-project-settings");
    let app_root = temp_root.join("app");
    let legacy_refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&legacy_refine_dir).unwrap();
    git(&app_root, &["init", "-q"]).unwrap();
    let refine_dir =
        crate::tools::host::project_layout::refine_dir_for_target_root(&app_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/project/status".to_string(),
        body: None,
    });
    assert_eq!(status.status, 200, "{:#}", status.body);
    assert_eq!(status.body["attached"], true);
    assert_eq!(status.body["target_root"], app_root.display().to_string());
    assert_eq!(status.body["apps"].as_array().unwrap().len(), 1);
    assert!(runtime_root.join("apps.json").exists());
    assert!(!temp_root.join("run/apps.json").exists());

    let app_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/apps/status".to_string(),
        body: None,
    });
    assert_eq!(app_status.status, 200);
    assert_eq!(app_status.body["attached"], true);

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "old-target-app-process".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: None,
            state: "running".to_string(),
            label: Some("sh".to_string()),
            details: Some("-c old target app".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let other_app = temp_root.join("other");
    fs::create_dir_all(&other_app).unwrap();
    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": other_app.display().to_string()})),
    });
    assert_eq!(attached.status, 200);
    assert_eq!(
        attached.body["target_root"],
        other_app.display().to_string()
    );
    assert!(supervisor.inspect("old-target-app-process").is_err());
    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body["attached"], true);

    let switched = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(switched.status, 200);
    assert_eq!(switched.body["target_root"], app_root.display().to_string());

    let third_app = temp_root.join("third");
    fs::create_dir_all(&third_app).unwrap();
    git(&third_app, &["init", "-q"]).unwrap();
    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/register".to_string(),
        body: Some(json!({
            "name": "third-app",
            "path": third_app.display().to_string()
        })),
    });
    assert_eq!(registered.status, 201);
    assert_eq!(registered.body["apps"].as_array().unwrap().len(), 3);

    let clone_source = temp_root.join("clone-source");
    let clone_destination = temp_root.join("clone-destination");
    fs::create_dir_all(&clone_source).unwrap();
    let output = Command::new("git")
        .arg("init")
        .arg(&clone_source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cloned = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/clone".to_string(),
        body: Some(json!({
            "source": clone_source.display().to_string(),
            "destination": clone_destination.display().to_string(),
            "name": "cloned-app",
            "make_current": false
        })),
    });
    assert_eq!(cloned.status, 201);
    assert!(clone_destination.join(".git").exists());
    assert_eq!(cloned.body["apps"].as_array().unwrap().len(), 4);

    let switched_by_name = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        body: Some(json!({"name": "third-app"})),
    });
    assert_eq!(switched_by_name.status, 200);
    assert_eq!(
        switched_by_name.body["target_root"],
        third_app.display().to_string()
    );

    let detached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/detach".to_string(),
        body: None,
    });
    assert_eq!(detached.status, 200);
    assert_eq!(detached.body["attached"], false);
    assert_eq!(detached.body["target_root"], serde_json::Value::Null);

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/apps".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["apps"].as_array().unwrap().len(), 4);
    assert_eq!(listed.body["current"], "");

    let settings = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/settings".to_string(),
        body: None,
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "claude");
    assert_eq!(settings.body["runtime"]["paused"], false);

    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "agent_cli": "smoke-ai",
            "parallel_run_cap": 3,
            "paused": true
        })),
    });
    assert_eq!(updated.status, 200);
    assert_eq!(updated.body["settings"]["agent_cli"], "smoke-ai");
    assert_eq!(updated.body["settings"]["parallel_run_cap"], "3");
    assert!(updated.body["settings"].get("paused").is_none());
    assert_eq!(updated.body["runtime"]["paused"], true);
    assert_eq!(updated.body["runtime"]["workflow_paused"], true);
    assert_eq!(updated.body["runtime"]["agents_paused"], true);
    assert_eq!(
        updated.body["runtime"]["background_processes_stopped"],
        true
    );
    assert!(runtime_root.join("process-control.json").exists());
    assert!(refine_dir.join("nodes.json").exists());
    assert!(!refine_dir.join("settings.json").exists());

    let removed = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/apps".to_string(),
        body: Some(json!({"path": other_app.display().to_string()})),
    });
    assert_eq!(removed.status, 200);
    assert_eq!(removed.body["apps"].as_array().unwrap().len(), 3);

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_project_attach_creates_missing_local_project() {
    let temp_root = unique_temp_dir("http-project-create-local");
    let destination = temp_root.join("new-app");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());

    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": destination.display().to_string()})),
    });

    assert_eq!(attached.status, 200);
    assert_eq!(
        attached.body["target_root"],
        destination.display().to_string()
    );
    assert!(destination.join(".git").exists());
    assert!(
        refine_dir_for_target_root(&destination)
            .unwrap()
            .join("refine.json")
            .exists()
    );
    assert!(!destination.join(".refine").exists());
    assert!(runtime_root.join("processes").exists());
    assert!(!destination.join(".refine/runtime/processes").exists());

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_applies_runtime_settings_updates_immediately() {
    let temp_root = unique_temp_dir("http-runtime-settings-apply");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        let created = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": format!("Instant runtime settings {id}")})),
        });
        assert_eq!(created.status, 201);
    }

    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "backlog_promote_after_seconds": "0"
        })),
    });
    assert_eq!(updated.status, 200);
    assert_eq!(updated.body["settings"]["parallel_run_cap"], "2");
    assert_eq!(
        updated.body["settings"]["backlog_promote_after_seconds"],
        "0"
    );

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["policy"]["global_limit"], 2);
    assert_eq!(state["policy"]["per_node_limit"], 2);
    assert_eq!(state["claims"].as_array().unwrap().len(), 2);

    let raised = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "parallel_run_cap": 3,
            "parallel_per_node_cap": 3
        })),
    });
    assert_eq!(raised.status, 200);
    assert_eq!(raised.body["settings"]["parallel_run_cap"], "3");

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["policy"]["global_limit"], 3);
    assert_eq!(state["policy"]["per_node_limit"], 3);
    assert_eq!(state["claims"].as_array().unwrap().len(), 3);

    let goal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(goal.status, 200);
    assert_eq!(goal.body["goal"]["status"], "todo");

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_worktree_cleanup_routes_to_the_attached_target_app() {
    let temp_root = unique_temp_dir("http-worktree-cleanup");
    let app_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&app_root);
    let refine_dir = refine_dir_for_target_root(&app_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Clean terminal worktree", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Tester", "Implement")
        .unwrap();
    work_items
        .set_goal_branch_name("GOAL1", "refine/GOAL1/round-1")
        .unwrap();
    work_items.cancel_goal_summary("GOAL1").unwrap();
    let worktree = app_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
    fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &app_root,
        &[
            "worktree",
            "add",
            "-b",
            "refine/GOAL1/round-1",
            worktree.to_str().unwrap(),
        ],
    )
    .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(runtime_root);
    let preview = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/worktrees/cleanup".to_string(),
        body: Some(json!({"apply": false})),
    });
    assert_eq!(preview.status, 200, "{:#}", preview.body);
    assert_eq!(preview.body["eligible"], 1);
    assert_eq!(preview.body["removed"], 0);
    assert!(worktree.exists());

    let applied = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/worktrees/cleanup".to_string(),
        body: Some(json!({"apply": true})),
    });
    assert_eq!(applied.status, 200, "{:#}", applied.body);
    assert_eq!(applied.body["removed"], 1);
    assert_eq!(applied.body["branches_deleted"], 0);
    assert!(!worktree.exists());
    assert!(
        git(
            &app_root,
            &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"]
        )
        .is_ok()
    );

    remove_temp_dir(&temp_root);
}
