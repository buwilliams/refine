use super::super::*;

pub(crate) fn project_status_is_attached_to_test_app(fixture: &IntegrationFixture) {
    let output = fixture.run_refine(&["project", "status"]);
    fixture.assert_success("project status", &output);
    let payload = fixture.json_stdout(&output);
    assert_eq!(payload["attached"], true, "{payload:#}");
    assert!(
        payload["target_root"]
            .as_str()
            .unwrap_or_default()
            .ends_with("rust-test-app"),
        "{payload:#}"
    );
    assert_eq!(payload["schema"]["compatible"], true, "{payload:#}");
}

pub(crate) fn daemon_backed_project_status_suppresses_ambiguous_default_label(
    fixture: &IntegrationFixture,
) {
    let refine_dir =
        refine::tools::host::project_layout::refine_dir_for_target_root(&fixture.app_root).unwrap();
    let path = refine_dir.join(refine::tools::product::nodes::NODE_REGISTRY_FILE);
    let original = fs::read(&path).ok();
    let mut registry = original
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .unwrap_or_else(|| {
            json!({
                "nodes": [{
                    "id": "default",
                    "display_name": "Default",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z"
                }]
            })
        });
    let default = registry["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == "default")
        .unwrap();
    default["display_name"] = json!("BO2LNXNEVO04 (QA)");
    default
        .as_object_mut()
        .unwrap()
        .remove("display_name_authority");
    fs::write(&path, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();

    let output = fixture.run_refine(&["project", "status"]);
    fixture.assert_success("project status with stale default label", &output);
    let payload = fixture.json_stdout(&output);
    assert_eq!(payload["active_node_id"], "default", "{payload:#}");
    assert_eq!(payload["active_node"], "Default", "{payload:#}");
    assert_eq!(
        payload["active_node_diagnostics"][0]["code"], "ambiguous_legacy_default_display_name",
        "{payload:#}"
    );

    match original {
        Some(bytes) => fs::write(path, bytes).unwrap(),
        None => fs::remove_file(path).unwrap(),
    }
}

pub(crate) fn project_doctor_runs(fixture: &IntegrationFixture) {
    let initial = fixture.run_refine(&["project", "doctor"]);
    fixture.assert_success("initial project doctor", &initial);
    let initial = fixture.json_stdout(&initial);
    assert_eq!(
        initial["processes"]["runner_reachable"], false,
        "{initial:#}"
    );

    let runtime_root = fixture.runtime_root.join(fixture.port.to_string());
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "cli-test-workflow-runner".to_string(),
            owner: ProcessOwner::Runner,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("CLI test workflow runner".to_string()),
            details: Some(json!({"kind": "runner", "worker_kind": "workflow"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = fixture.run_refine(&["project", "doctor"]);
        fixture.assert_success("project doctor", &output);
        let payload = fixture.json_stdout(&output);
        if payload["processes"]["runner_reachable"] == true {
            assert!(
                payload["processes"]["process_count"]
                    .as_u64()
                    .is_some_and(|count| count >= 2),
                "{payload:#}"
            );
            assert_eq!(
                payload["processes"]["running_process_count"],
                payload["processes"]["process_count"],
                "{payload:#}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "project doctor kept stale startup process health: {payload:#}"
        );
        thread::sleep(Duration::from_millis(100));
    }
    fs::remove_file(
        supervisor
            .processes_dir()
            .join("cli-test-workflow-runner.json"),
    )
    .unwrap();
}

pub(crate) fn project_registry_lifecycle_commands(fixture: &IntegrationFixture) {
    let primary_app = fixture.app_root.display().to_string();
    let registered_app = fixture.create_git_app("rust-registered-app");
    let registered_app_path = registered_app.display().to_string();
    let clone_source = fixture.create_git_app("rust-clone-source");
    let clone_destination = fixture.app_workspace_root().join("rust-cloned-app");
    let _ = fs::remove_dir_all(&clone_destination);
    let clone_destination_path = clone_destination.display().to_string();

    let register = fixture.run_refine(&["project", "register", "registered", &registered_app_path]);
    fixture.assert_success("project register", &register);
    let register_payload = fixture.json_stdout(&register);
    assert_eq!(register_payload["ok"], true);
    assert!(project_apps(&register_payload).iter().any(|app| {
        app["name"].as_str() == Some("registered")
            && app["path"].as_str() == Some(registered_app_path.as_str())
    }));

    let switch = fixture.run_refine(&["project", "switch", "registered"]);
    fixture.assert_success("project switch", &switch);
    let switch_payload = fixture.json_stdout(&switch);
    assert_eq!(switch_payload["attached"], true);
    assert_eq!(switch_payload["target_root"], registered_app_path);

    let detach = fixture.run_refine(&["project", "detach"]);
    fixture.assert_success("project detach", &detach);
    let detach_payload = fixture.json_stdout(&detach);
    assert_eq!(detach_payload["attached"], false);
    assert!(
        detach_payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("No refine project is attached")
    );

    let attach = fixture.run_refine(&["project", "attach", &primary_app]);
    fixture.assert_success("project attach primary", &attach);
    assert_eq!(fixture.json_stdout(&attach)["target_root"], primary_app);

    let migrate = fixture.run_refine(&["project", "migrate"]);
    fixture.assert_success("project migrate", &migrate);
    let migrate_payload = fixture.json_stdout(&migrate);
    assert_eq!(migrate_payload["ok"], true);
    assert_eq!(migrate_payload["migrated"], false);

    let sync = fixture.run_refine(&["sync"]);
    fixture.assert_success("sync", &sync);
    assert!(fixture.json_stdout(&sync).is_object());

    let clone = fixture.run_refine(&[
        "project",
        "clone",
        clone_source.to_str().unwrap(),
        &clone_destination_path,
        "--name",
        "cloned",
        "--make-current",
    ]);
    fixture.assert_success("project clone", &clone);
    let clone_payload = fixture.json_stdout(&clone);
    assert_eq!(clone_payload["attached"], true);
    assert_eq!(clone_payload["target_root"], clone_destination_path);

    let restore = fixture.run_refine(&["project", "switch", &primary_app]);
    fixture.assert_success("project switch primary", &restore);
    assert_eq!(fixture.json_stdout(&restore)["target_root"], primary_app);

    let remove_registered = fixture.run_refine(&["project", "remove", "registered"]);
    fixture.assert_success("project remove registered", &remove_registered);
    assert!(
        !project_apps(&fixture.json_stdout(&remove_registered))
            .iter()
            .any(|app| app["name"].as_str() == Some("registered"))
    );

    let remove_cloned = fixture.run_refine(&["project", "remove", "cloned"]);
    fixture.assert_success("project remove cloned", &remove_cloned);
    assert!(
        !project_apps(&fixture.json_stdout(&remove_cloned))
            .iter()
            .any(|app| app["name"].as_str() == Some("cloned"))
    );
}

pub(crate) fn project_apps(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    payload["apps"]
        .as_array()
        .cloned()
        .or_else(|| payload["apps"]["apps"].as_array().cloned())
        .unwrap_or_default()
}
