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

#[test]
fn retired_backlog_promotion_is_removed_without_validating_legacy_values() {
    for legacy in [
        json!("0"),
        json!("3600"),
        json!("-1"),
        json!("invalid"),
        json!({"bad": true}),
    ] {
        let root =
            std::env::temp_dir().join(format!("refine-retired-backlog-{}", uuid::Uuid::new_v4()));
        let nodes = FileNodeRegistryService::new(&root);
        nodes.create("worker").unwrap();
        nodes.create("other").unwrap();
        let mut registry = nodes.load_registry().unwrap();
        for node in &mut registry.nodes {
            node.settings
                .insert("backlog_promote_after_seconds".into(), legacy.clone());
            node.settings
                .insert("agent_idle_timeout_seconds".into(), json!("123"));
        }
        nodes.save_registry(&registry).unwrap();
        let other_before = registry
            .nodes
            .iter()
            .find(|node| node.id == "other")
            .unwrap()
            .clone();
        let service = FileSettingsService::for_node(&root, "worker");
        let loaded = service.load().unwrap();
        assert!(!loaded.contains_key("backlog_promote_after_seconds"));
        assert_eq!(loaded["agent_idle_timeout_seconds"], "123");
        let persisted = nodes.load_registry().unwrap();
        let worker = persisted
            .nodes
            .iter()
            .find(|node| node.id == "worker")
            .unwrap();
        assert!(
            !worker
                .settings
                .contains_key("backlog_promote_after_seconds")
        );
        assert_eq!(worker.settings["agent_idle_timeout_seconds"], "123");
        assert_eq!(
            persisted
                .nodes
                .iter()
                .find(|node| node.id == "other")
                .unwrap(),
            &other_before
        );
        assert_eq!(service.load().unwrap(), loaded);
        let error = service
            .update(&json!({"backlog_promote_after_seconds": legacy}))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown setting: backlog_promote_after_seconds")
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn runtime_copy_discards_retired_backlog_promotion_on_both_nodes() {
    let root = std::env::temp_dir().join(format!("refine-copy-backlog-{}", uuid::Uuid::new_v4()));
    let nodes = FileNodeRegistryService::new(&root);
    nodes.create("source").unwrap();
    nodes.create("destination").unwrap();
    let mut registry = nodes.load_registry().unwrap();
    for node in &mut registry.nodes {
        if node.id == "source" || node.id == "destination" {
            node.settings.insert(
                "backlog_promote_after_seconds".into(),
                json!({"invalid": true}),
            );
            node.settings.insert(
                "target_app_url".into(),
                json!(format!("https://{}.invalid", node.id)),
            );
            node.settings.insert(
                "parallel_run_cap".into(),
                json!(if node.id == "source" { "3" } else { "1" }),
            );
        }
    }
    nodes.save_registry(&registry).unwrap();

    let destination = FileSettingsService::for_node(&root, "destination");
    let copied = destination.copy_from_node("source", "runtime").unwrap();
    assert_eq!(copied["settings"]["parallel_run_cap"], "3");
    assert_eq!(
        copied["settings"]["target_app_url"],
        "https://destination.invalid"
    );
    assert!(
        copied["settings"]
            .get("backlog_promote_after_seconds")
            .is_none()
    );
    for node in nodes.load_registry().unwrap().nodes {
        if node.id == "source" || node.id == "destination" {
            assert!(!node.settings.contains_key("backlog_promote_after_seconds"));
            assert_eq!(node.settings["parallel_run_cap"], "3");
            assert_eq!(
                node.settings["target_app_url"],
                format!("https://{}.invalid", node.id)
            );
        }
    }
    assert_eq!(
        destination.copy_from_node("source", "runtime").unwrap()["copied_count"],
        0
    );
    fs::remove_dir_all(root).unwrap();
}
