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
fn active_selection_has_one_machine_local_location_shared_by_every_resolver() {
    // The selection must resolve identically no matter how the service is
    // constructed: a daemon holding a port root and a daemonless CLI holding
    // only the project reached different files here once, and one of those
    // files replicated over git sync — pinning the whole fleet to a single
    // node identity and failing every other node's goals at Quality.
    let temp_root = unique_temp_dir("nodes-active-root");
    let refine_dir = temp_root.join("app/.refine");
    let run_a = temp_root.join("run/8082");
    let run_b = temp_root.join("run/8083");
    let daemon_a = FileNodeRegistryService::with_active_root(&refine_dir, &run_a);
    let daemon_b = FileNodeRegistryService::with_active_root(&refine_dir, &run_b);
    let daemonless = FileNodeRegistryService::new(&refine_dir);

    daemon_a.create("node-a").unwrap();
    daemon_a.create("node-b").unwrap();
    daemon_a.rename("node-a", "Ethan").unwrap();
    daemon_a.activate("node-a").unwrap();

    assert_eq!(daemon_a.active_node_id().unwrap(), "node-a");
    assert_eq!(daemon_b.active_node_id().unwrap(), "node-a");
    assert_eq!(daemonless.active_node_id().unwrap(), "node-a");
    assert_eq!(daemon_a.active_identity().unwrap().display_name, "Ethan");

    // The selection lives under the sync-excluded runtime subtree, never at
    // the synchronized state root and never in a per-daemon port root.
    assert!(refine_dir.join("runtime/active-node.json").exists());
    assert!(!refine_dir.join("active-node.json").exists());
    assert!(!run_a.join("active-node.json").exists());
    assert!(!run_b.join("active-node.json").exists());

    daemonless.activate("node-b").unwrap();
    assert_eq!(daemon_a.active_node_id().unwrap(), "node-b");
    assert_eq!(daemon_b.active_node_id().unwrap(), "node-b");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn legacy_port_root_selection_migrates_into_the_canonical_location() {
    let temp_root = unique_temp_dir("nodes-active-migration");
    let refine_dir = temp_root.join("app/.refine");
    let port_root = temp_root.join("run/8082");
    let service = FileNodeRegistryService::with_active_root(&refine_dir, &port_root);
    service.create("worker").unwrap();
    fs::create_dir_all(&port_root).unwrap();
    fs::write(
        port_root.join(ACTIVE_NODE_FILE),
        serde_json::json!({
            "active_node_id": "worker",
            "refine_dir": refine_dir.display().to_string(),
            "updated_at": "2026-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    let identity = service.active_identity().unwrap();
    assert_eq!(identity.id, "worker");
    assert!(identity.diagnostics.is_empty());
    // Adopted into the canonical location and retired from the port root, so
    // a daemonless resolver agrees from now on.
    assert!(refine_dir.join("runtime/active-node.json").exists());
    assert!(!port_root.join(ACTIVE_NODE_FILE).exists());
    assert_eq!(
        FileNodeRegistryService::new(&refine_dir)
            .active_node_id()
            .unwrap(),
        "worker"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn synced_root_selection_is_never_read_and_yields_a_diagnostic() {
    // A root-level active-node.json is exactly what git sync replicated
    // fleet-wide; resolution must ignore it even when nothing else exists.
    let temp_root = unique_temp_dir("nodes-synced-selection");
    let refine_dir = temp_root.join("app/.refine");
    let service = FileNodeRegistryService::new(&refine_dir);
    service.create("intruder").unwrap();
    fs::write(
        refine_dir.join(ACTIVE_NODE_FILE),
        serde_json::json!({
            "active_node_id": "intruder",
            "refine_dir": refine_dir.display().to_string(),
            "updated_at": "2026-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    let identity = service.active_identity().unwrap();
    assert_eq!(identity.id, "default");
    assert_eq!(
        identity.diagnostics[0].code,
        "synced_active_node_selection_ignored"
    );

    // Recording a real local selection retires the untrusted root copy.
    service.activate("intruder").unwrap();
    assert_eq!(service.active_node_id().unwrap(), "intruder");
    assert!(!refine_dir.join(ACTIVE_NODE_FILE).exists());

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

    // Selections are project-scoped by location now, so activating in one
    // project cannot leak into another sharing the same daemon runtime root.
    assert_eq!(service_a.active_node_id().unwrap(), "ethan");
    assert_eq!(service_b.active_node_id().unwrap(), "default");

    // A pre-canonical port-root selection recorded for a different project is
    // refused at migration instead of adopted.
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        runtime_root.join(ACTIVE_NODE_FILE),
        serde_json::json!({
            "active_node_id": "qa",
            "refine_dir": refine_a.display().to_string(),
            "updated_at": "2026-01-01T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();
    let mismatched = service_b.active_identity().unwrap();
    assert_eq!(mismatched.id, "default");
    assert_eq!(
        mismatched.diagnostics[0].code,
        "active_node_selection_project_mismatch"
    );
    // The refused source must not be adopted into project B's canonical file.
    assert!(!refine_b.join("runtime/active-node.json").exists());

    service_b.activate("qa").unwrap();
    assert_eq!(service_b.active_node_id().unwrap(), "qa");
    assert_eq!(service_a.active_node_id().unwrap(), "ethan");

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

#[test]
fn hand_edited_uppercase_node_ids_normalize_and_stay_matchable() {
    // Registries are plain serde-parsed, so provisioning scripts have written
    // ids like `BO2LNXIPSAPP01` that `clean_node_id` would reject. Byte-exact
    // comparisons made such an owner invisible to its own node.
    let temp_root = unique_temp_dir("nodes-uppercase-id");
    let refine_dir = temp_root.join("app/.refine");
    write_legacy_registry(
        &refine_dir,
        serde_json::json!([legacy_node("BO2LNXIPSAPP01", "App Server")]),
    );
    let service = FileNodeRegistryService::new(&refine_dir);

    let response = service.list_response().unwrap();
    let ids: Vec<_> = response["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"bo2lnxipsapp01".to_string()), "ids: {ids:?}");
    service.activate("bo2lnxipsapp01").unwrap();
    assert_eq!(service.active_node_id().unwrap(), "bo2lnxipsapp01");

    // Goal records stamped with the original casing must still match.
    assert!(node_ids_match("BO2LNXIPSAPP01", "bo2lnxipsapp01"));
    assert!(!node_ids_match("bo2lnxipsapp01", "bo2lnxnevo03-buddy"));

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
