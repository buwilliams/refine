use super::*;

#[test]
fn daemon_sync_operation_following_returns_terminal_results_and_structured_errors() {
    let completed = follow_daemon_operation_with(
        json!({"operation": {
            "id": "op-complete",
            "status": "complete",
            "result": {"git_sync": {"ok": true}}
        }}),
        Duration::from_secs(1),
        |_| unreachable!("terminal operation must not poll"),
    )
    .unwrap();
    assert_eq!(completed["git_sync"]["ok"], true);

    for terminal in ["failed", "cancelled", "interrupted"] {
        let error = follow_daemon_operation_with(
            json!({"operation": {
                "id": format!("op-{terminal}"),
                "status": terminal,
                "result": {},
                "error": {
                    "code": "project_sync_failed",
                    "message": "conflict",
                    "conflict_report_location": "/run/conflicts/latest.json"
                }
            }}),
            Duration::from_secs(1),
            |_| unreachable!("terminal operation must not poll"),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains(terminal), "{message}");
        assert!(message.contains("conflict_report_location"), "{message}");
    }

    let timed_out = follow_daemon_operation_with(
        json!({"operation": {
            "id": "op-timeout",
            "status": "running",
            "result": {},
            "error": null
        }}),
        Duration::ZERO,
        |_| unreachable!("zero timeout must not poll"),
    )
    .unwrap_err();
    assert!(timed_out.to_string().contains("operation_timed_out"));
}

#[test]
fn sync_rebuilds_projection_from_cli_surface() {
    let temp_root = unique_temp_dir("cli-project-sync");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "CLI visible Goal",
              "status": "done",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();

    let cli = Cli::try_parse_from([
        "refine",
        "sync",
        "--target-root",
        target_root.to_str().unwrap(),
        "--cache-dir",
        cache_dir.to_str().unwrap(),
    ])
    .unwrap();
    dispatch(cli).unwrap();

    assert!(cache_dir.join(PROJECTION_SNAPSHOT_FILE).exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_registry_commands_use_shared_file_project_registry_service() {
    let temp_root = unique_temp_dir("cli-project-registry");
    let runtime_root = temp_root.join("run");
    let app_one = temp_root.join("app-one");
    let app_two = temp_root.join("app-two");
    fs::create_dir_all(app_one.join(".refine")).unwrap();
    fs::create_dir_all(app_two.join(".refine")).unwrap();
    git_init(&app_one);
    git_init(&app_two);

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "status",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--target-root",
            app_one.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry_path = runtime_root.join("apps.json");
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert_eq!(registry["active_app"], app_one.to_str().unwrap());

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "detach",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert!(registry["active_app"].is_null());

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "attach",
            app_one.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "register",
            "second",
            app_two.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "switch",
            "second",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert_eq!(registry["active_app"], app_two.to_str().unwrap());

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "remove",
            "second",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert!(registry["apps"].get(app_two.to_str().unwrap()).is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn in_process_project_status_suppresses_ambiguous_legacy_default_label() {
    let temp_root = unique_temp_dir("cli-project-status-node-identity");
    let runtime_root = temp_root.join("run/8082");
    let app_root = temp_root.join("app");
    fs::create_dir_all(&app_root).unwrap();
    git_init(&app_root);
    let refine_dir = refine_dir_for_target_root(&app_root).unwrap();
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join(crate::application::fleet::nodes::NODE_REGISTRY_FILE),
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
    let service = crate::application::projects::registry::FileProjectRegistryService::new(
        &runtime_root,
        Some(app_root.clone()),
    );
    let status = service.status().unwrap();
    assert_eq!(status.active_node_id.as_deref(), Some("default"));
    assert_eq!(status.active_node.as_deref(), Some("Default"));
    assert_eq!(
        status.active_node_diagnostics[0].code,
        "ambiguous_legacy_default_display_name"
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "status",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--target-root",
            app_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_clone_uses_shared_file_project_registry_service() {
    let temp_root = unique_temp_dir("cli-project-clone");
    let runtime_root = temp_root.join("run");
    let source = temp_root.join("source");
    let destination = temp_root.join("clone-destination");
    fs::create_dir_all(&source).unwrap();
    let output = std::process::Command::new("git")
        .arg("init")
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "clone",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--name",
            "cloned",
            "--make-current",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    assert!(destination.join(".git").exists());
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(runtime_root.join("apps.json")).unwrap()).unwrap();
    assert_eq!(registry["active_app"], destination.to_str().unwrap());
    assert_eq!(
        registry["apps"][destination.to_str().unwrap()]["name"],
        "cloned"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_attach_creates_missing_local_project() {
    let temp_root = unique_temp_dir("cli-project-create-local");
    let runtime_root = temp_root.join("run");
    let destination = temp_root.join("new-app");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "attach",
            destination.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    assert!(destination.join(".git").exists());
    assert!(
        refine_dir_for_target_root(&destination)
            .unwrap()
            .join("refine.json")
            .exists()
    );
    assert!(!destination.join(".refine").exists());
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(runtime_root.join("apps.json")).unwrap()).unwrap();
    assert_eq!(registry["active_app"], destination.to_str().unwrap());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_attach_requires_an_agent_for_legacy_refine_migration() {
    let temp_root = unique_temp_dir("cli-project-migration");
    let runtime_root = temp_root.join("run");
    let app_root = temp_root.join("legacy-app");
    let target_root = app_root.clone();
    let refine_dir = target_root.join(".refine");
    fs::create_dir_all(refine_dir.join("gaps/GA")).unwrap();
    fs::write(refine_dir.join("gaps/GA/gap.json"), "{}").unwrap();

    let attach_error = dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "attach",
            app_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(attach_error.contains("migration agent"));
    assert!(!refine_dir.join("refine.json").exists());
    assert!(refine_dir.join("gaps/GA/gap.json").exists());

    let migrate_error = dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "migrate",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(migrate_error.contains("migration agent"));

    fs::remove_dir_all(temp_root).unwrap();
}
