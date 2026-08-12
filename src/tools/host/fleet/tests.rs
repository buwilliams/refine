use super::*;
use crate::process::supervisor::config::FileSettingsService;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn bootstrap_remote_node_builds_dry_run_ssh_command() {
    let result = bootstrap_remote_node(FleetBootstrapRequest {
        node_id: "node-1".to_string(),
        ssh_host: "example.com".to_string(),
        ssh_user: "deploy".to_string(),
        ssh_identity_path: "~/.ssh/refine_ed25519".to_string(),
        ssh_port: 2222,
        refine_checkout: "~/refine".to_string(),
        target_app_path: "/srv/app".to_string(),
        refine_port: 8081,
        dry_run: true,
    })
    .unwrap();
    assert!(result.ok);
    assert_eq!(result.exit_code, None);
    assert!(result.command.contains("ssh -p 2222"));
    assert!(result.command.contains("-o BatchMode=yes"));
    assert!(result.command.contains("-o ConnectTimeout=10"));
    assert!(result.command.contains("-o ServerAliveCountMax=2"));
    assert!(
        result
            .command
            .contains("-o StrictHostKeyChecking=accept-new")
    );
    assert!(result.command.contains("-o LogLevel=ERROR"));
    assert!(
        result
            .command
            .contains("-o 'UserKnownHostsFile=run/fleet-processes/fleet-known_hosts'")
    );
    assert!(result.command.contains("-i '~/.ssh/refine_ed25519'"));
    assert!(result.command.contains("'deploy@example.com'"));
    assert!(result.remote_command.contains("refine_port=8081"));
    assert!(result.remote_command.contains("/srv/app"));
}

#[test]
fn bootstrap_remote_node_rejects_user_at_host() {
    let error = bootstrap_remote_node(FleetBootstrapRequest {
        node_id: "node-1".to_string(),
        ssh_host: "user@example.com".to_string(),
        ssh_user: String::new(),
        ssh_identity_path: String::new(),
        ssh_port: 22,
        refine_checkout: String::new(),
        target_app_path: String::new(),
        refine_port: 8082,
        dry_run: true,
    })
    .unwrap_err();
    assert!(matches!(error, RefineError::InvalidInput(_)));
}

#[test]
fn ssh_preflight_reports_missing_identity_file() {
    let temp_root = unique_temp_dir("fleet-ssh-preflight");
    let missing_identity = temp_root.join("missing_ed25519");

    let error = validate_ssh_prerequisites(missing_identity.to_str().unwrap()).unwrap_err();

    assert!(matches!(error, RefineError::InvalidInput(_)));
    assert!(error.to_string().contains("ssh identity file"));
}

#[test]
fn ssh_command_uses_existing_identity_file() {
    let temp_root = unique_temp_dir("fleet-ssh-command");
    fs::create_dir_all(&temp_root).unwrap();
    let identity = temp_root.join("id_ed25519");
    fs::write(&identity, "").unwrap();

    let command = ssh_process_command(
        2222,
        "deploy",
        "example.com",
        identity.to_str().unwrap(),
        "printf ok",
        None,
    )
    .unwrap();

    let args = command.args;
    assert!(args.contains(&"BatchMode=yes".to_string()));
    assert!(args.contains(&"ConnectTimeout=10".to_string()));
    assert!(args.contains(&"-i".to_string()));
    assert!(args.contains(&identity.display().to_string()));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_fleet_service_manages_node_lifecycle() {
    let temp_root = unique_temp_dir("fleet");
    let refine_dir = temp_root.join(".refine");
    let service = FileFleetService::new(&refine_dir);

    assert_eq!(service.list_response().unwrap()["enabled"], true);
    service.add_node("node-1").unwrap();
    service.set_enabled("node-1", false).unwrap();
    assert_eq!(service.show("node-1").unwrap()["node"]["enabled"], false);
    service.set_enabled("node-1", true).unwrap();
    service.transfer("GOAL1", "node-1").unwrap();
    service.sync().unwrap();
    service.maintenance_response().unwrap();
    service.remove_node("node-1").unwrap();
    assert!(
        service
            .registry()
            .unwrap()
            .nodes
            .iter()
            .all(|node| node.id != "node-1")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_fleet_service_migrates_legacy_fleet_json_to_nodes() {
    let temp_root = unique_temp_dir("fleet-legacy-migration");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join("cluster.json"),
        serde_json::json!({
            "nodes": [{
                "id": "node-1",
                "display_name": "Legacy Node",
                "ssh_host": "example.com",
                "ssh_user": "deploy",
                "ssh_identity_path": "~/.ssh/refine_ed25519",
                "ssh_port": 2222,
                "refine_checkout": "/srv/refine",
                "target_app_path": "/srv/app",
                "refine_port": 18081,
                "enabled": true,
                "health": null,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z"
            }],
            "updated_at": "2026-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    let service = FileFleetService::new(&refine_dir);
    let response = service.list_response().unwrap();
    let migrated_node = response["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(migrated_node["ssh_host"], "example.com");
    assert_eq!(migrated_node["ssh_port"], 2222);
    let nodes_path = refine_dir.join("nodes.json");
    let first_nodes = fs::read_to_string(&nodes_path).unwrap();

    service.list_response().unwrap();
    let second_nodes = fs::read_to_string(&nodes_path).unwrap();
    assert_eq!(first_nodes, second_nodes);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn fleet_node_api_uses_the_shared_identity_contract() {
    let temp_root = unique_temp_dir("fleet-default-identity");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join(crate::tools::product::nodes::NODE_REGISTRY_FILE),
        serde_json::json!({
            "nodes": [{
                "id": "default",
                "display_name": "QA Host",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z"
            }]
        })
        .to_string(),
    )
    .unwrap();
    let service = FileFleetService::new(&refine_dir);

    let ambiguous = service.list_response().unwrap();
    assert_eq!(ambiguous["nodes"][0]["display_name"], "Default");
    assert_eq!(ambiguous["nodes"][0]["registry_display_name"], "QA Host");
    assert_eq!(
        ambiguous["nodes"][0]["identity_diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );

    let confirmed = service
        .upsert_node(
            "default",
            NodeRemoteUpdate {
                display_name: Some("Review Node".to_string()),
                ..NodeRemoteUpdate::default()
            },
        )
        .unwrap();
    assert_eq!(confirmed["nodes"][0]["display_name"], "Review Node");
    assert!(
        confirmed["nodes"][0]["identity_diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_fleet_service_authorizes_remote_run_commands() {
    let temp_root = unique_temp_dir("fleet-security");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&serde_json::json!({"allowed_commands": "printf"}))
        .unwrap();
    let service = FileFleetService::with_runtime_root(&refine_dir, &runtime_root);
    service
        .upsert_node(
            "node-1",
            NodeRemoteUpdate {
                ssh_host: Some("example.com".to_string()),
                ssh_user: Some("deploy".to_string()),
                ssh_identity_path: Some("~/.ssh/refine_ed25519".to_string()),
                enabled: Some(true),
                ..NodeRemoteUpdate::default()
            },
        )
        .unwrap();

    let denied = service.run_remote_response("node-1", "rm -rf target");

    assert!(matches!(denied, Err(RefineError::Unauthorized(_))));
    let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
    assert!(audit.contains("\"outcome\":\"denied\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_targets_only_enabled_healthy_nodes() {
    let temp_root = unique_temp_dir("fleet-distribute");
    let refine_dir = temp_root.join(".refine");
    let service = FileFleetService::new(&refine_dir);
    service.add_node("worker-up").unwrap();
    service.add_node("worker-down").unwrap();
    service.add_node("worker-broken").unwrap();
    service.set_enabled("worker-down", false).unwrap();
    {
        let registry_service = FileNodeRegistryService::new(&refine_dir);
        let mut registry = registry_service.load_registry().unwrap();
        let broken = registry
            .nodes
            .iter_mut()
            .find(|node| node.id == "worker-broken")
            .unwrap();
        broken.health = Some(FleetHealth {
            status: "failed".to_string(),
            checked_at: now_timestamp(),
            details: None,
        });
        registry_service.save_registry(&registry).unwrap();
    }
    crate::tools::product::work_items::FileWorkItemService::new(&refine_dir)
        .create_goal_summary("Distributable", Some("GOAL1"))
        .unwrap();

    let response = service.distribute_response(None, false, true).unwrap();
    let node_ids: Vec<&str> = response["distribute"]["node_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(node_ids.contains(&"default"));
    assert!(node_ids.contains(&"worker-up"));
    assert!(!node_ids.contains(&"worker-down"));
    assert!(!node_ids.contains(&"worker-broken"));

    let converge_error = service.distribute_response(None, true, true).unwrap_err();
    assert!(matches!(converge_error, RefineError::InvalidInput(_)));

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
