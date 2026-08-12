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
