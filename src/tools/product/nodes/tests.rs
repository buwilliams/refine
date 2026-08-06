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
    service_a.rename("node-a", "Ethan").unwrap();
    service_a.rename("node-b", "QA").unwrap();
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
    assert_eq!(service_a.active_identity().unwrap().display_name, "Ethan");
    assert_eq!(service_b.active_identity().unwrap().display_name, "QA");
    assert!(refine_dir.join("nodes.json").exists());
    assert!(!refine_dir.join("active-node.json").exists());
    assert!(run_a.join("active-node.json").exists());
    assert!(run_b.join("active-node.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn stale_legacy_default_label_is_diagnostic_until_explicitly_confirmed() {
    let temp_root = unique_temp_dir("nodes-stale-default-label");
    let refine_dir = temp_root.join("app/.refine");
    let runtime_root = temp_root.join("run/8082");
    write_legacy_registry(
        &refine_dir,
        serde_json::json!([
            legacy_node("default", "BO2LNXNEVO04 (QA)"),
            legacy_node("qa", "Quality Assurance"),
        ]),
    );
    let service = FileNodeRegistryService::with_active_root(&refine_dir, &runtime_root);

    let response = service.list_response().unwrap();
    assert_eq!(response["active_node_id"], "default");
    assert_eq!(response["active_node"], "Default");
    assert_eq!(
        response["diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );
    let default = response["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "default")
        .unwrap();
    assert_eq!(default["display_name"], "Default");
    assert_eq!(default["registry_display_name"], "BO2LNXNEVO04 (QA)");
    assert_eq!(
        default["identity_diagnostics"][0]["code"],
        "ambiguous_legacy_default_display_name"
    );
    let qa = response["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "qa")
        .unwrap();
    assert_eq!(qa["display_name"], "Quality Assurance");

    service.rename("default", "Local Workstation").unwrap();
    let confirmed = service.list_response().unwrap();
    assert_eq!(confirmed["active_node"], "Local Workstation");
    assert!(confirmed["diagnostics"].as_array().unwrap().is_empty());
    assert_eq!(confirmed["nodes"][0]["display_name_authority"], "user");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn runtime_selection_is_scoped_to_the_attached_project_across_restart() {
    let temp_root = unique_temp_dir("nodes-project-selection");
    let refine_a = temp_root.join("app-a/.refine");
    let refine_b = temp_root.join("app-b/.refine");
    let runtime_root = temp_root.join("run/8082");
    let service_a = FileNodeRegistryService::with_active_root(&refine_a, &runtime_root);
    let service_b = FileNodeRegistryService::with_active_root(&refine_b, &runtime_root);
    service_a.create("ethan").unwrap();
    service_b.create("qa").unwrap();
    service_a.activate("ethan").unwrap();

    assert_eq!(service_a.active_node_id().unwrap(), "ethan");
    let switched = service_b.active_identity().unwrap();
    assert_eq!(switched.id, "default");
    assert_eq!(
        switched.diagnostics[0].code,
        "active_node_selection_project_mismatch"
    );

    let restarted = FileNodeRegistryService::with_active_root(&refine_b, &runtime_root)
        .active_identity()
        .unwrap();
    assert_eq!(restarted.id, "default");
    assert_eq!(
        restarted.diagnostics[0].code,
        "active_node_selection_project_mismatch"
    );

    service_b.activate("qa").unwrap();
    assert_eq!(service_b.active_node_id().unwrap(), "qa");
    assert_eq!(service_a.active_node_id().unwrap(), "default");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn legacy_unscoped_runtime_selection_requires_reactivation() {
    let temp_root = unique_temp_dir("nodes-legacy-active-selection");
    let refine_dir = temp_root.join("app/.refine");
    let runtime_root = temp_root.join("run/8082");
    let service = FileNodeRegistryService::with_active_root(&refine_dir, &runtime_root);
    service.create("worker").unwrap();
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        runtime_root.join(ACTIVE_NODE_FILE),
        serde_json::json!({"active_node_id": "worker"}).to_string(),
    )
    .unwrap();

    let legacy = service.active_identity().unwrap();
    assert_eq!(legacy.id, "default");
    assert_eq!(
        legacy.diagnostics[0].code,
        "legacy_unscoped_active_node_selection"
    );
    service.activate("worker").unwrap();
    assert_eq!(service.active_node_id().unwrap(), "worker");

    fs::remove_dir_all(temp_root).unwrap();
}

fn legacy_node(id: &str, display_name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "display_name": display_name,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    })
}

fn write_legacy_registry(refine_dir: &Path, nodes: serde_json::Value) {
    fs::create_dir_all(refine_dir).unwrap();
    fs::write(
        refine_dir.join(NODE_REGISTRY_FILE),
        serde_json::json!({"nodes": nodes}).to_string(),
    )
    .unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
