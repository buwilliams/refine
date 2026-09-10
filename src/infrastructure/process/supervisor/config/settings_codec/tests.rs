use super::*;

#[test]
fn auto_approve_defaults_false_for_existing_nodes_and_validates_boolean_updates() {
    let root = std::env::temp_dir().join(format!("refine-auto-approve-{}", uuid::Uuid::new_v4()));
    let nodes = FileNodeRegistryService::new(&root);
    nodes.create("worker").unwrap();
    let service = FileSettingsService::for_node(&root, "worker");
    assert_eq!(service.load().unwrap()["auto_approve"], "false");
    assert!(
        nodes.settings("worker").unwrap()["settings"]
            .get("auto_approve")
            .is_none()
    );

    for value in [json!(true), json!(false), json!("true"), json!("false")] {
        let expected = value
            .as_str()
            .map(str::to_string)
            .unwrap_or(value.to_string());
        let saved = service.update(&json!({"auto_approve": value})).unwrap();
        assert_eq!(saved["settings"]["auto_approve"], expected);
        assert_eq!(service.load().unwrap()["auto_approve"], expected);
    }
    let before = fs::read(service.path()).unwrap();
    for value in [
        json!(null),
        json!(1),
        json!(0),
        json!("yes"),
        json!(""),
        json!([]),
        json!({}),
    ] {
        let error = service.update(&json!({"auto_approve": value})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("auto_approve must be true or false")
        );
        assert_eq!(fs::read(service.path()).unwrap(), before);
    }
    service.update(&json!({"auto_approve": true})).unwrap();
    // Copying from an existing node with an absent flag restores the false default.
    let copied = service.copy_from_node("default", "runtime").unwrap();
    assert_eq!(copied["settings"]["auto_approve"], "false");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn auto_approve_rejects_invalid_stored_values_without_rewriting_them() {
    let root = std::env::temp_dir().join(format!(
        "refine-auto-approve-invalid-{}",
        uuid::Uuid::new_v4()
    ));
    let nodes = FileNodeRegistryService::new(&root);
    nodes.create("worker").unwrap();
    let mut registry = nodes.load_registry().unwrap();
    registry
        .nodes
        .iter_mut()
        .find(|node| node.id == "worker")
        .unwrap()
        .settings
        .insert("auto_approve".to_string(), json!("invalid"));
    nodes.save_registry(&registry).unwrap();
    let service = FileSettingsService::for_node(&root, "worker");
    let before = fs::read(service.path()).unwrap();
    assert!(service.load().is_err());
    assert_eq!(fs::read(service.path()).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}
