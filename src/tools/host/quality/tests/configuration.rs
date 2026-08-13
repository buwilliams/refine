use super::*;

#[test]
fn quality_settings_persist_and_report_configured_state() {
    let temp_root = unique_temp_dir("quality-settings");
    let refine_dir = temp_root.join(".refine");
    let service = FileQualityService::new(&refine_dir);

    let saved = service
        .save_settings(QualitySettingsPatch {
            business_requirements: Some("Must load dashboard".to_string()),
            instructions: Some("Run focused checks".to_string()),
            tests: Some(vec!["Dashboard loads".to_string()]),
            enabled: Some(json!("1")),
            timing: Some("post_build".to_string()),
        })
        .unwrap();

    assert_eq!(saved.enabled, "1");
    assert!(saved.configured);
    assert_eq!(saved.tests, vec!["Dashboard loads"]);
    assert!(refine_dir.join(SETTINGS_FILE).exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_evaluation_fails_when_agent_omits_a_configured_test() {
    let result = parse_quality_provider_output(
        "GOAL1",
        &["First outcome".to_string(), "Second outcome".to_string()],
        r#"{"ok":true,"summary":"Done","results":[{"test":"First outcome","status":"passed","evidence":"Observed","command":""}]}"#,
    )
    .unwrap();

    assert!(!result.ok);
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[1].test, "Second outcome");
    assert_eq!(result.results[1].status, "failed");
    assert!(result.results[1].evidence.contains("omitted"));
}

#[test]
fn quality_migrates_enabled_legacy_commands_without_a_silent_noop() {
    let temp_root = unique_temp_dir("quality-legacy-migration");
    let refine_dir = temp_root.join("state");
    fs::create_dir_all(&refine_dir).unwrap();
    let default_commands = serde_json::to_string(&json!([
        {"command": "printf legacy-one", "enabled": true},
        {"command": "printf disabled", "enabled": false}
    ]))
    .unwrap();
    let inactive_commands = serde_json::to_string(&json!([
        {"command": "printf legacy-one", "enabled": true},
        {"command": "printf legacy-two", "enabled": true}
    ]))
    .unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        serde_json::to_string_pretty(&json!({"nodes": [
            legacy_quality_node("default", "post_build", &default_commands),
            legacy_quality_node("inactive", "post_build", &inactive_commands)
        ]}))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(refine_dir.join("quality")).unwrap();
    fs::write(
        refine_dir.join(SETTINGS_FILE),
        serde_json::to_string_pretty(&json!({
            "timing": "post_build",
            "migration_version": 2
        }))
        .unwrap(),
    )
    .unwrap();

    let service = FileQualityService::new(&refine_dir);
    let migrated = service.load_settings().unwrap();
    assert_eq!(
        migrated.legacy_commands,
        vec!["printf legacy-one", "printf legacy-two"]
    );
    assert!(migrated.configured);
    let nodes: Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    assert!(
        nodes["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["settings"].get("quality_timing").is_none())
    );

    assert!(
        FileSettingsService::new(&refine_dir)
            .update(&json!({"quality_timing": "pre_merge"}))
            .is_err()
    );
    assert!(
        FileSettingsService::new(&refine_dir)
            .load()
            .unwrap()
            .get("quality_timing")
            .is_none()
    );
    let quality_settings = fs::read_to_string(refine_dir.join(SETTINGS_FILE)).unwrap();
    assert!(!quality_settings.contains("\"timing\""));

    let transitioned = service
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Replacement behavior passes".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    assert_eq!(transitioned.tests, vec!["Replacement behavior passes"]);
    assert!(transitioned.legacy_commands.is_empty());
    fs::remove_dir_all(temp_root).unwrap();
}
