use super::*;
use crate::infrastructure::process::supervisor::config::FileSettingsService;
use std::collections::BTreeMap;
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
fn bootstrap_health_settlement_preserves_settings_written_after_the_request_snapshot() {
    let temp_root = unique_temp_dir("fleet-bootstrap-settlement-merge");
    let refine_dir = temp_root.join(".refine");
    let service = FileFleetService::new(&refine_dir);
    service.add_node("node-1").unwrap();

    let request = service
        .nodes()
        .with_registry_lock(|| service.bootstrap_node_request_locked("node-1", true))
        .unwrap();
    FileSettingsService::for_node(&refine_dir, "node-1")
        .update(&serde_json::json!({
            "automatic_agent_resource_budget_percent": 44
        }))
        .unwrap();

    let result = RemoteRunResult {
        node_id: request.node_id.clone(),
        command: "ssh node-1".to_string(),
        remote_command: "bootstrap".to_string(),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        ok: true,
    };
    service
        .nodes()
        .with_registry_lock(|| service.settle_bootstrap_node_response_locked(&request, result))
        .unwrap();

    let settings = FileSettingsService::for_node(&refine_dir, "node-1")
        .list_response()
        .unwrap();
    assert_eq!(
        settings["settings"]["automatic_agent_resource_budget_percent"],
        "44"
    );
    assert_eq!(
        service.show("node-1").unwrap()["node"]["health"]["status"],
        "ready"
    );

    fs::remove_dir_all(temp_root).unwrap();
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
        refine_dir.join(crate::application::fleet::nodes::NODE_REGISTRY_FILE),
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
    crate::application::work_items::FileWorkItemService::new(&refine_dir)
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

/// A fleet upgrades node by node, so one node's daemon can still be on the
/// previous API contract while the rest of the fleet is current. That node is
/// A node whose Git is too old to run the state merge is that node's own
/// condition, exactly like a node awaiting its build upgrade: it gets its own
/// status rather than collapsing into a generic failure, every other node
/// still syncs, and the pass still succeeds. The status is read from the
/// stable `error.reason`, never from the message prose.
#[test]
fn a_node_with_an_unsupported_git_is_that_nodes_own_condition() {
    let temp_root = unique_temp_dir("fleet-unsupported-git");
    let refine_dir = temp_root.join(".refine");
    let service = FileFleetService::new(&refine_dir);
    for id in ["worker-old-git", "worker-fine", "worker-broken"] {
        service
            .upsert_node(
                id,
                NodeRemoteUpdate {
                    ssh_host: Some("example.com".to_string()),
                    refine_port: Some(8082),
                    ..NodeRemoteUpdate::default()
                },
            )
            .unwrap();
    }

    let client = ScriptedNodeDaemons::new([
        (
            "worker-old-git",
            NodeDaemonReply::Answered {
                status: 503,
                body: serde_json::json!({
                    "error": {
                        "code": "unsupported_git_version",
                        "reason": "unsupported_git_version",
                        "message": "Refine needs Git 2.42 or newer to synchronize state, but this node has 2.34."
                    }
                }),
            },
        ),
        (
            "worker-fine",
            NodeDaemonReply::Answered {
                status: 200,
                body: serde_json::json!({"operation": {"id": "op-1", "status": "running"}}),
            },
        ),
        // A 503 without the reason is still just a failure.
        (
            "worker-broken",
            NodeDaemonReply::Answered {
                status: 503,
                body: serde_json::json!({"error": {"message": "degraded"}}),
            },
        ),
    ]);

    let report = service.sync_nodes_with(&client).unwrap();
    let status = |id: &str| {
        report
            .nodes
            .iter()
            .find(|node| node.node_id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the fleet sync report"))
            .clone()
    };
    let old_git = status("worker-old-git");
    assert_eq!(old_git.status, NODE_SYNC_UNSUPPORTED_GIT);
    assert!(
        old_git
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("2.42"),
        "the operator needs the version sentence: {old_git:#?}"
    );
    assert_eq!(status("worker-fine").status, NODE_SYNC_QUEUED);
    assert_eq!(status("worker-broken").status, NODE_SYNC_FAILED);

    // One node's Git never becomes the fleet's problem, and it never rewrites
    // the node's provisioning health.
    let fleet = service.registry().unwrap();
    let node = fleet
        .nodes
        .iter()
        .find(|node| node.id == "worker-old-git")
        .unwrap();
    assert!(node.health.is_none(), "{node:#?}");

    std::fs::remove_dir_all(&temp_root).ok();
}

/// reported as `pending_upgrade` — its own status, carrying both contract
/// versions — while every other node still syncs, the pass still succeeds,
/// and the not-yet-upgraded node stays eligible for work.
#[test]
fn a_node_rejecting_the_api_contract_is_that_nodes_pending_upgrade() {
    let temp_root = unique_temp_dir("fleet-pending-upgrade");
    let refine_dir = temp_root.join(".refine");
    let service = FileFleetService::new(&refine_dir);
    for id in ["worker-old", "worker-new", "worker-off", "worker-gone"] {
        service
            .upsert_node(
                id,
                NodeRemoteUpdate {
                    ssh_host: Some("example.com".to_string()),
                    refine_port: Some(8082),
                    ..NodeRemoteUpdate::default()
                },
            )
            .unwrap();
    }
    service.set_enabled("worker-off", false).unwrap();

    let client = ScriptedNodeDaemons::new([
        // The previous build's daemon answers the version gate, naming the
        // contract it speaks.
        (
            "worker-old",
            NodeDaemonReply::Answered {
                status: 426,
                body: serde_json::json!({
                    "error": {
                        "code": "api_version_mismatch",
                        "message": "unsupported Refine API contract version"
                    },
                    "api_contract_version": "2",
                    "supported_api_contract_versions": ["2"]
                }),
            },
        ),
        (
            "worker-new",
            NodeDaemonReply::Answered {
                status: 200,
                body: serde_json::json!({"operation": {"id": "op-1", "status": "running"}}),
            },
        ),
        (
            "worker-gone",
            NodeDaemonReply::Unreachable("connection refused".to_string()),
        ),
    ]);

    let report = service.sync_nodes_with(&client).unwrap();

    let status = |id: &str| {
        report
            .nodes
            .iter()
            .find(|node| node.node_id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the fleet sync report"))
            .clone()
    };
    let old = status("worker-old");
    assert_eq!(old.status, NODE_SYNC_PENDING_UPGRADE);
    assert_eq!(old.api_contract_version.as_deref(), Some("2"));
    assert_eq!(
        old.expected_api_contract_version,
        crate::application::protocol::API_CONTRACT_VERSION
    );
    assert!(
        old.detail
            .as_deref()
            .unwrap_or_default()
            .contains("upgrade"),
        "{old:#?}"
    );
    assert_eq!(status("worker-new").status, NODE_SYNC_QUEUED);
    assert_eq!(status("worker-off").status, NODE_SYNC_DISABLED);
    assert_eq!(status("worker-gone").status, NODE_SYNC_UNREACHABLE);
    assert_eq!(status("default").status, NODE_SYNC_LOCAL);
    assert_eq!(report.pending_upgrade, vec!["worker-old".to_string()]);

    // A node waiting its turn in a rolling upgrade is a working node: it
    // keeps its work and keeps receiving more.
    let fleet = service.registry().unwrap();
    let node = |id: &str| {
        fleet
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the fleet"))
            .clone()
    };
    assert!(node_health_allows_distribution(&node("worker-old")));

    // `nodes.json` is synchronized state and `health` is the node's
    // provisioning verdict, so a fleet sync writes neither: one node's
    // two-second observation of another node's daemon is neither shared
    // truth nor a reason to withhold work.
    let registry_path = FileNodeRegistryService::new(&refine_dir).registry_path();
    let before = fs::read(&registry_path).unwrap();
    let repeated = service.sync_nodes_with(&client).unwrap();
    assert_eq!(repeated.pending_upgrade, vec!["worker-old".to_string()]);
    assert_eq!(fs::read(&registry_path).unwrap(), before);
    assert!(
        fleet.nodes.iter().all(|node| node.health.is_none()),
        "fleet sync must not write node health: {:#?}",
        fleet.nodes
    );

    fs::remove_dir_all(temp_root).unwrap();
}

/// A bootstrap verdict is the one thing that withholds work from a node, and
/// it survives every fleet sync: an unreachable daemon does not clear a
/// failed provisioning, and an erroring daemon does not manufacture one.
#[test]
fn fleet_sync_never_rewrites_the_provisioning_verdict_that_gates_work() {
    let temp_root = unique_temp_dir("fleet-sync-keeps-bootstrap-verdict");
    let refine_dir = temp_root.join(".refine");
    let service = FileFleetService::new(&refine_dir);
    for id in ["worker-broken", "worker-healthy"] {
        service
            .upsert_node(
                id,
                NodeRemoteUpdate {
                    ssh_host: Some("example.com".to_string()),
                    refine_port: Some(8082),
                    ..NodeRemoteUpdate::default()
                },
            )
            .unwrap();
    }
    let mut registry = FileNodeRegistryService::new(&refine_dir)
        .load_registry()
        .unwrap();
    for node in registry.nodes.iter_mut() {
        node.health = Some(FleetHealth {
            status: if node.id == "worker-broken" {
                "failed"
            } else {
                "ready"
            }
            .to_string(),
            checked_at: "2026-08-17T08:00:00Z".to_string(),
            details: Some(
                serde_json::json!({"bootstrap": {"ok": node.id != "worker-broken"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        });
    }
    FileNodeRegistryService::new(&refine_dir)
        .save_registry(&registry)
        .unwrap();

    let client = ScriptedNodeDaemons::new([
        (
            "worker-broken",
            NodeDaemonReply::Unreachable("connection refused".to_string()),
        ),
        (
            "worker-healthy",
            NodeDaemonReply::Answered {
                status: 500,
                body: serde_json::json!({"error": {"message": "state conflict"}}),
            },
        ),
    ]);

    let report = service.sync_nodes_with(&client).unwrap();
    let status = |id: &str| {
        report
            .nodes
            .iter()
            .find(|node| node.node_id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the fleet sync report"))
            .status
            .clone()
    };
    assert_eq!(status("worker-broken"), NODE_SYNC_UNREACHABLE);
    assert_eq!(status("worker-healthy"), NODE_SYNC_FAILED);

    let fleet = service.registry().unwrap();
    let node = |id: &str| {
        fleet
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the fleet"))
            .clone()
    };
    // The failed bootstrap still names its reason and still withholds work.
    let broken = node("worker-broken").health.unwrap();
    assert_eq!(broken.status, "failed");
    assert_eq!(broken.details.unwrap()["bootstrap"]["ok"], false);
    assert!(!node_health_allows_distribution(&node("worker-broken")));
    // A node whose daemon answered an error is still a provisioned node.
    assert_eq!(node("worker-healthy").health.unwrap().status, "ready");
    assert!(node_health_allows_distribution(&node("worker-healthy")));

    fs::remove_dir_all(temp_root).unwrap();
}

/// Scripted per-node daemon replies, so the classification is exercised
/// without a network.
struct ScriptedNodeDaemons {
    replies: BTreeMap<String, NodeDaemonReply>,
}

impl ScriptedNodeDaemons {
    fn new<const N: usize>(replies: [(&str, NodeDaemonReply); N]) -> Self {
        Self {
            replies: replies
                .into_iter()
                .map(|(id, reply)| (id.to_string(), reply))
                .collect(),
        }
    }
}

impl FleetNodeDaemonClient for ScriptedNodeDaemons {
    fn sync_node(&self, node: &Node) -> NodeDaemonReply {
        self.replies
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| NodeDaemonReply::Unreachable(format!("no stub for {}", node.id)))
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
