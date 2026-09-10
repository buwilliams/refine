use super::*;

#[test]
fn retired_email_approval_is_ignored_and_cannot_be_configured() {
    let root = std::env::temp_dir().join(format!("refine-retired-email-{}", uuid::Uuid::new_v4()));
    let nodes = FileNodeRegistryService::new(&root);
    nodes.create("worker").unwrap();
    let mut registry = nodes.load_registry().unwrap();
    registry
        .nodes
        .iter_mut()
        .find(|node| node.id == "worker")
        .unwrap()
        .settings
        .insert("auto_approve".into(), json!("invalid"));
    nodes.save_registry(&registry).unwrap();
    let service = FileSettingsService::for_node(&root, "worker");
    assert!(!service.load().unwrap().contains_key("auto_approve"));
    assert!(service.update(&json!({"auto_approve": true})).is_err());
    fs::remove_dir_all(root).unwrap();
}
