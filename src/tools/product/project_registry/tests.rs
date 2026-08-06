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
    let runtime_root = temp_root.join("run/8082");
    let app_a = temp_root.join("app-a");
    let app_b = temp_root.join("app-b");
    fs::create_dir_all(&app_a).unwrap();
    fs::create_dir_all(&app_b).unwrap();
    git_init(&app_a);
    git_init(&app_b);
    let service = FileProjectRegistryService::new(&runtime_root, None);
    service.attach(app_a.to_str().unwrap()).unwrap();
    service
        .register_path(Some("app-b"), app_b.to_str().unwrap(), false)
        .unwrap();
    let refine_a = refine_dir_for_target_root(&app_a).unwrap();
    let refine_b = refine_dir_for_target_root(&app_b).unwrap();
    let nodes_a = FileNodeRegistryService::with_active_root(&refine_a, &runtime_root);
    nodes_a.create("ethan").unwrap();
    nodes_a.rename("ethan", "Ethan's Node").unwrap();
    nodes_a.activate("ethan").unwrap();

    let active_a = service.status().unwrap();
    assert_eq!(active_a.active_node_id.as_deref(), Some("ethan"));
    assert_eq!(active_a.active_node.as_deref(), Some("Ethan's Node"));
    assert!(active_a.active_node_diagnostics.is_empty());

    fs::create_dir_all(&refine_b).unwrap();
    FileProjectMigrationService::new(&refine_b)
        .initialize_current_schema()
        .unwrap();
    fs::write(
        refine_b.join(crate::tools::product::nodes::NODE_REGISTRY_FILE),
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
    let switched = service.switch_with_migration("app-b").unwrap();
    assert_eq!(switched.active_node_id.as_deref(), Some("default"));
    assert_eq!(switched.active_node.as_deref(), Some("Default"));
    let codes = switched
        .active_node_diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"active_node_selection_project_mismatch"));
    assert!(codes.contains(&"ambiguous_legacy_default_display_name"));

    let restarted = FileProjectRegistryService::new(&runtime_root, None)
        .status()
        .unwrap();
    assert_eq!(restarted.active_node_id.as_deref(), Some("default"));
    assert_eq!(restarted.active_node.as_deref(), Some("Default"));
    assert_eq!(restarted.active_node_diagnostics.len(), 2);

    fs::remove_dir_all(temp_root).unwrap();
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
