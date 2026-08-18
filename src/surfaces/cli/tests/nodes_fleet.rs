use super::*;

#[test]
fn node_commands_use_shared_node_registry_service() {
    let temp_root = unique_temp_dir("cli-node-registry");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Owned Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();

    for argv in [
        vec![
            "refine",
            "node",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "create",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "rename",
            "node-1",
            "Node One",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "activate",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "settings",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "activate",
            "default",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "archive",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let goal = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(goal.contains("\"node_id\": \"node-1\""));
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    assert_eq!(nodes["nodes"][1]["display_name"], "Node One");
    assert_eq!(nodes["nodes"][1]["archived"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn fleet_commands_use_shared_fleet_service() {
    let temp_root = unique_temp_dir("cli-fleet-registry");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Fleet Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    // `fleet sync` asks every other node's daemon to sync; this suite has no
    // fleet to reach, so the transport is stubbed instead of dialled.
    let _daemons = install_node_daemon_client(
        &refine_dir,
        std::sync::Arc::new(StubNodeDaemon(NodeDaemonReply::Unreachable(
            "no daemon in tests".to_string(),
        ))),
    );

    for argv in [
        vec![
            "refine",
            "fleet",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "add-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "show",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "edit-node",
            "node-1",
            "--ssh-host",
            "example.com",
            "--ssh-user",
            "deploy",
            "--ssh-identity-path",
            "~/.ssh/refine_ed25519",
            "--ssh-port",
            "2222",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "disable-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "enable-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "bootstrap",
            "node-1",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "edit-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "distribute",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "distribute",
            "--to",
            "node-1",
            "--converge",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "sync",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "maintenance",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "fleet",
            "remove-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let goal = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(goal.contains("\"node_id\": \"node-1\""));
    assert!(!refine_dir.join("cluster.json").exists());
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    let node = nodes["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(node["ssh_host"], "example.com");
    assert_eq!(node["ssh_user"], "deploy");
    assert_eq!(node["ssh_port"], 2222);
    assert_eq!(node["archived"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

/// `refine fleet sync` during a rolling upgrade: the node still on the
/// previous build rejects this build's API contract, and that is reported as
/// that node's pending upgrade. The command succeeds, this node's own state
/// still syncs, and the rest of the fleet is still asked.
#[test]
fn fleet_sync_reports_a_not_yet_upgraded_node_as_pending_upgrade() {
    let temp_root = unique_temp_dir("cli-fleet-sync-pending-upgrade");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Fleet Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    for id in ["worker-old", "worker-new"] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "fleet",
                "edit-node",
                id,
                "--ssh-host",
                "example.com",
                "--refine-port",
                "8082",
                "--target-root",
                target_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    }

    let _daemons = install_node_daemon_client(
        &refine_dir,
        std::sync::Arc::new(PerNodeStubDaemon {
            pending_upgrade: "worker-old",
        }),
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "fleet",
            "sync",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .expect("one node awaiting its upgrade must not fail the fleet sync");

    // The per-node statuses are this pass's report, not a rewrite of the
    // fleet's registry: a node's recorded health is its provisioning verdict
    // and no sync answer may touch it.
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    for node in nodes["nodes"].as_array().unwrap() {
        assert!(
            node["health"].is_null(),
            "fleet sync must not write node health: {node:#}"
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

/// A standing conflict on this node is this node's condition. `fleet sync`
/// still asks every other node — an operator confirming a rollout must be
/// able to read the fleet's statuses — and still fails on this node's own
/// leg afterwards.
#[test]
fn fleet_sync_asks_every_node_even_when_this_nodes_own_sync_fails() {
    let temp_root = unique_temp_dir("cli-fleet-sync-local-failure");
    let target_root = temp_root.clone();
    fs::create_dir_all(&target_root).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["remote", "add", "origin", "/refine/no/such/remote.git"],
    ] {
        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(&target_root)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
    }
    // A real repository keeps its state outside the checkout, so ask for the
    // same directory the command will use rather than assuming `.refine`.
    let refine_dir = crate::tools::host::project_layout::prepare_refine_dir(&target_root).unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Fleet Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "fleet",
            "add-node",
            "worker-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "fleet",
            "edit-node",
            "worker-1",
            "--ssh-host",
            "example.com",
            "--refine-port",
            "8082",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _daemons = install_node_daemon_client(
        &refine_dir,
        std::sync::Arc::new(CountingStubDaemon {
            calls: calls.clone(),
        }),
    );

    let result = dispatch(
        Cli::try_parse_from([
            "refine",
            "fleet",
            "sync",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    );

    assert!(
        result.is_err(),
        "this node's own unreachable remote still fails the command"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "every other node is asked even when this node's own pass failed"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

/// Counts the nodes asked, so a fan-out that never happened is visible.
struct CountingStubDaemon {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl FleetNodeDaemonClient for CountingStubDaemon {
    fn sync_node(&self, _node: &crate::model::node::Node) -> NodeDaemonReply {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        NodeDaemonReply::Answered {
            status: 202,
            body: json!({"operation": {"id": "op-1", "status": "running"}}),
        }
    }
}

/// One canned reply for every node.
struct StubNodeDaemon(NodeDaemonReply);

impl FleetNodeDaemonClient for StubNodeDaemon {
    fn sync_node(&self, _node: &crate::model::node::Node) -> NodeDaemonReply {
        self.0.clone()
    }
}

/// The named node answers the API-contract gate the way a daemon on the
/// previous build does; every other node accepts the request.
struct PerNodeStubDaemon {
    pending_upgrade: &'static str,
}

impl FleetNodeDaemonClient for PerNodeStubDaemon {
    fn sync_node(&self, node: &crate::model::node::Node) -> NodeDaemonReply {
        if node.id == self.pending_upgrade {
            return NodeDaemonReply::Answered {
                status: 426,
                body: json!({
                    "error": {
                        "code": "api_version_mismatch",
                        "message": "unsupported Refine API contract version"
                    },
                    "api_contract_version": "2",
                    "supported_api_contract_versions": ["2"]
                }),
            };
        }
        NodeDaemonReply::Answered {
            status: 200,
            body: json!({"operation": {"id": "op-1", "status": "running"}}),
        }
    }
}

#[test]
fn fleet_free_text_and_distribute_instructions_parse_as_agent_requests() {
    let cli = Cli::try_parse_from(["refine", "fleet", "group related goals on one node"]).unwrap();
    match cli.command {
        Commands::Fleet {
            action: FleetAction::Request(words),
        } => assert_eq!(words, vec!["group related goals on one node"]),
        other => panic!("expected fleet request, parsed {other:?}"),
    }

    let cli = Cli::try_parse_from([
        "refine",
        "fleet",
        "distribute",
        "spread the backlog evenly, keeping related goals together",
    ])
    .unwrap();
    match cli.command {
        Commands::Fleet {
            action:
                FleetAction::Distribute {
                    instructions: Some(instructions),
                    ..
                },
        } => assert_eq!(
            instructions,
            "spread the backlog evenly, keeping related goals together"
        ),
        other => panic!("expected agent-directed distribute, parsed {other:?}"),
    }

    // Instructions and deterministic flags are mutually exclusive.
    assert!(
        Cli::try_parse_from([
            "refine",
            "fleet",
            "distribute",
            "instructions",
            "--to",
            "node"
        ])
        .is_err()
    );

    // A single bare word is treated as a mistyped subcommand, not a request.
    let error = dispatch(Cli::try_parse_from(["refine", "fleet", "lst"]).unwrap()).unwrap_err();
    assert!(error.to_string().contains("unknown fleet command"));
}
