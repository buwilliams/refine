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
fn cluster_commands_use_shared_cluster_service() {
    let temp_root = unique_temp_dir("cli-cluster-registry");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Cluster Goal",
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
            "cluster",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "add-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "show",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
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
            "cluster",
            "disable-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "enable-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "bootstrap",
            "node-1",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "edit-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "distribute",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
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
            "cluster",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "sync",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "maintenance",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
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
