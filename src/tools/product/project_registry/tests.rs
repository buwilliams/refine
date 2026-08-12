use super::*;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn normalize_app_path_expands_home_prefix() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };

    assert_eq!(
        normalize_app_path("~/refine-test-app").unwrap(),
        home.join("refine-test-app")
    );
}

#[test]
fn file_project_registry_persists_apps_and_active_status() {
    let temp_root = unique_temp_dir("project-registry");
    let runtime_root = temp_root.join("run/8080");
    let app_root = temp_root.join("app");
    fs::create_dir_all(app_root.join(".refine")).unwrap();
    git_init(&app_root);
    let service = FileProjectRegistryService::new(&runtime_root, Some(app_root.clone()));

    let status = service.status().unwrap();
    assert!(status.attached);
    assert_eq!(status.apps.apps.len(), 1);
    assert_eq!(
        status.apps.active_app.as_deref(),
        Some(app_root.to_str().unwrap())
    );
    assert!(service.path().exists());

    let listed = service.list_response().unwrap();
    assert_eq!(listed["apps"].as_array().unwrap().len(), 1);

    service.detach().unwrap();
    assert!(service.load().unwrap().active_app.is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_project_registry_clones_and_registers_app() {
    let temp_root = unique_temp_dir("project-registry-clone");
    let runtime_root = temp_root.join("run/8080");
    let source = temp_root.join("source");
    let destination = temp_root.join("cloned-app");
    fs::create_dir_all(&source).unwrap();
    let output = Command::new("git")
        .arg("init")
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let service = FileProjectRegistryService::new(&runtime_root, None);
    let status = service
        .clone_app(
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            Some("cloned"),
            true,
        )
        .unwrap();
    assert!(destination.join(".git").exists());
    assert_eq!(
        status.target_root.as_deref(),
        Some(destination.to_str().unwrap())
    );
    let registry = service.load().unwrap();
    assert_eq!(
        registry.active_app.as_deref(),
        Some(destination.to_str().unwrap())
    );
    assert_eq!(
        registry
            .apps
            .get(destination.to_str().unwrap())
            .unwrap()
            .name,
        "cloned"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_project_registry_attach_creates_missing_local_project() {
    let temp_root = unique_temp_dir("project-registry-create-local");
    let runtime_root = temp_root.join("run/8080");
    let destination = temp_root.join("new-app");
    let service = FileProjectRegistryService::new(&runtime_root, None);

    let status = service.attach(destination.to_str().unwrap()).unwrap();

    assert_eq!(
        status.target_root.as_deref(),
        Some(destination.to_str().unwrap())
    );
    assert!(destination.join(".git").exists());
    let refine_dir = refine_dir_for_target_root(&destination).unwrap();
    assert!(refine_dir.join("refine.json").exists());
    assert!(!destination.join(".refine").exists());
    assert!(runtime_root.join("processes").exists());
    assert!(!destination.join(".refine/runtime/processes").exists());
    let registry = service.load().unwrap();
    assert_eq!(
        registry.active_app.as_deref(),
        Some(destination.to_str().unwrap())
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_project_registry_normalizes_refine_dir_inputs_before_persisting() {
    let temp_root = unique_temp_dir("project-registry-refine-dir");
    let runtime_root = temp_root.join("run/8080");
    let app_root = temp_root.join("app");
    fs::create_dir_all(app_root.join(".refine")).unwrap();
    git_init(&app_root);
    FileProjectMigrationService::new(app_root.join(".refine"))
        .initialize_current_schema()
        .unwrap();
    let service = FileProjectRegistryService::new(&runtime_root, None);

    let status = service
        .attach(app_root.join(".refine").to_str().unwrap())
        .unwrap();

    assert_eq!(
        status.target_root.as_deref(),
        Some(app_root.to_str().unwrap())
    );
    let registry = service.load().unwrap();
    assert_eq!(
        registry.active_app.as_deref(),
        Some(app_root.to_str().unwrap())
    );
    assert!(registry.apps.contains_key(app_root.to_str().unwrap()));
    assert!(
        !registry
            .apps
            .contains_key(app_root.join(".refine").to_str().unwrap())
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_status_uses_authoritative_identity_across_attach_switch_and_restart() {
    let temp_root = unique_temp_dir("project-registry-node-identity");
    let registry_root = temp_root.join("run");
    let active_node_root = registry_root.join("8082");
    let app_a = temp_root.join("app-a");
    let app_b = temp_root.join("app-b");
    let clone_source = temp_root.join("clone-source");
    let clone_destination = temp_root.join("clone-destination");
    fs::create_dir_all(&app_a).unwrap();
    fs::create_dir_all(&app_b).unwrap();
    fs::create_dir_all(&clone_source).unwrap();
    git_init(&app_a);
    git_init(&app_b);
    git_init(&clone_source);
    let service = FileProjectRegistryService::new(&registry_root, None)
        .with_active_node_root(&active_node_root);

    // Initialize app A, then deliberately split the stale shared selection from
    // the authoritative port-local selection.
    service.attach(app_a.to_str().unwrap()).unwrap();
    let refine_a = refine_dir_for_target_root(&app_a).unwrap();
    let shared_nodes = FileNodeRegistryService::with_active_root(&refine_a, &registry_root);
    shared_nodes.create("stale-base").unwrap();
    shared_nodes
        .rename("stale-base", "Stale Base Node")
        .unwrap();
    shared_nodes.activate("stale-base").unwrap();

    let port_nodes_a = FileNodeRegistryService::with_active_root(&refine_a, &active_node_root);
    port_nodes_a.create("port-a").unwrap();
    port_nodes_a.rename("port-a", "Port A").unwrap();
    port_nodes_a.activate("port-a").unwrap();

    let attached_a = service.attach(app_a.to_str().unwrap()).unwrap();
    assert_project_status_identity(&attached_a, "port-a", "Port A");
    let active_a = service.status().unwrap();
    assert_project_status_identity(&active_a, "port-a", "Port A");
    assert_eq!(shared_nodes.active_node_id().unwrap(), "stale-base");

    service
        .register_path(Some("app-b"), app_b.to_str().unwrap(), false)
        .unwrap();
    let switched_b = service.switch_with_migration("app-b").unwrap();
    assert_project_status_project_mismatch(&switched_b);
    let refine_b = refine_dir_for_target_root(&app_b).unwrap();
    let port_nodes_b = FileNodeRegistryService::with_active_root(&refine_b, &active_node_root);
    port_nodes_b.create("port-b").unwrap();
    port_nodes_b.rename("port-b", "Port B").unwrap();
    port_nodes_b.activate("port-b").unwrap();
    let inspected_b = service.inspect(app_b.to_str().unwrap()).unwrap();
    assert_project_status_identity(&inspected_b, "port-b", "Port B");

    let switched_a = service.switch_with_migration("app-a").unwrap();
    assert_project_status_project_mismatch(&switched_a);
    port_nodes_a.activate("port-a").unwrap();
    let inspected_a = service.inspect(app_a.to_str().unwrap()).unwrap();
    assert_project_status_identity(&inspected_a, "port-a", "Port A");

    let cloned = service
        .clone_app(
            clone_source.to_str().unwrap(),
            clone_destination.to_str().unwrap(),
            Some("cloned"),
            true,
        )
        .unwrap();
    assert_project_status_project_mismatch(&cloned);
    let clone_refine_dir = refine_dir_for_target_root(&clone_destination).unwrap();
    assert!(clone_refine_dir.join("refine.json").exists());
    let port_nodes_clone =
        FileNodeRegistryService::with_active_root(&clone_refine_dir, &active_node_root);
    port_nodes_clone.create("port-clone").unwrap();
    port_nodes_clone.rename("port-clone", "Port Clone").unwrap();
    port_nodes_clone.activate("port-clone").unwrap();
    let inspected_clone = service
        .inspect(clone_destination.to_str().unwrap())
        .unwrap();
    assert_project_status_identity(&inspected_clone, "port-clone", "Port Clone");

    let detached = service.detach().unwrap();
    assert!(!detached.attached);
    assert!(detached.active_node_id.is_none());
    assert!(detached.active_node.is_none());
    assert!(detached.active_node_diagnostics.is_empty());

    let mismatched_attach = service.attach(app_a.to_str().unwrap()).unwrap();
    assert_project_status_project_mismatch(&mismatched_attach);
    port_nodes_a.activate("port-a").unwrap();
    let reattached_a = service.attach(app_a.to_str().unwrap()).unwrap();
    assert_project_status_identity(&reattached_a, "port-a", "Port A");

    let restarted = FileProjectRegistryService::new(&registry_root, None)
        .with_active_node_root(&active_node_root)
        .status()
        .unwrap();
    assert_project_status_identity(&restarted, "port-a", "Port A");

    assert!(registry_root.join(APP_REGISTRY_FILE).exists());
    assert!(!active_node_root.join(APP_REGISTRY_FILE).exists());
    assert!(registry_root.join("processes").exists());
    assert!(!active_node_root.join("processes").exists());
    for refine_dir in [&refine_a, &refine_b, &clone_refine_dir] {
        assert!(refine_dir.join("refine.json").exists());
    }

    fs::remove_dir_all(temp_root).unwrap();
}

fn assert_project_status_identity(status: &ProjectStatus, id: &str, display_name: &str) {
    assert!(status.attached);
    assert_eq!(status.active_node_id.as_deref(), Some(id));
    assert_eq!(status.active_node.as_deref(), Some(display_name));
    assert!(status.active_node_diagnostics.is_empty());
    assert_ne!(status.active_node_id.as_deref(), Some("stale-base"));
    assert_ne!(status.active_node.as_deref(), Some("Stale Base Node"));
}

fn assert_project_status_project_mismatch(status: &ProjectStatus) {
    assert!(status.attached);
    assert_eq!(status.active_node_id.as_deref(), Some("default"));
    assert_eq!(status.active_node.as_deref(), Some("Default"));
    assert!(
        status
            .active_node_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "active_node_selection_project_mismatch")
    );
    assert_ne!(status.active_node_id.as_deref(), Some("stale-base"));
    assert_ne!(status.active_node.as_deref(), Some("Stale Base Node"));
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn git_init(root: &Path) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
