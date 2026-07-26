use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn file_node_registry_manages_nodes_and_active_selection() {
    let temp_root = unique_temp_dir("nodes");
    let refine_dir = temp_root.join(".refine");
    let service = FileNodeRegistryService::new(&refine_dir);

    assert_eq!(
        service.list_response().unwrap()["active_node_id"],
        "default"
    );
    service.create("node-1").unwrap();
    service.rename("node-1", "Node One").unwrap();
    service.activate("node-1").unwrap();
    assert_eq!(service.list_response().unwrap()["active_node_id"], "node-1");
    assert!(service.archive("node-1").is_err());

    service.activate("default").unwrap();
    service.archive("node-1").unwrap();
    assert_eq!(service.show("node-1").unwrap()["node"]["archived"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_node_registry_can_keep_active_selection_outside_shared_refine_dir() {
    let temp_root = unique_temp_dir("nodes-active-root");
    let refine_dir = temp_root.join("app/.refine");
    let run_a = temp_root.join("run/8082");
    let run_b = temp_root.join("run/8083");
    let service_a = FileNodeRegistryService::with_active_root(&refine_dir, &run_a);
    let service_b = FileNodeRegistryService::with_active_root(&refine_dir, &run_b);

    service_a.create("node-a").unwrap();
    service_a.create("node-b").unwrap();
    service_a.activate("node-a").unwrap();
    service_b.activate("node-b").unwrap();

    assert_eq!(
        service_a.list_response().unwrap()["active_node_id"],
        "node-a"
    );
    assert_eq!(
        service_b.list_response().unwrap()["active_node_id"],
        "node-b"
    );
    assert!(refine_dir.join("nodes.json").exists());
    assert!(!refine_dir.join("active-node.json").exists());
    assert!(run_a.join("active-node.json").exists());
    assert!(run_b.join("active-node.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
