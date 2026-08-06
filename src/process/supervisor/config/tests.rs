use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn file_settings_service_lists_defaults_and_persists_updates() {
    let temp_root = unique_temp_dir("settings");
    let refine_dir = temp_root.join(".refine");
    let service = FileSettingsService::new(&refine_dir);

    assert_eq!(service.load().unwrap()["agent_cli"], "claude");
    assert!(
        !service
            .load()
            .unwrap()
            .contains_key("development_request_email_enabled")
    );
    assert!(
        service
            .update(&serde_json::json!({"development_request_email_enabled": true}))
            .is_err()
    );
    let updated = service
        .update(&serde_json::json!({
            "agent_cli": "smoke-ai",
            "parallel_run_cap": 4,
            "target_app_env_json": {"PORT": 3000}
        }))
        .unwrap();
    assert_eq!(updated["settings"]["agent_cli"], "smoke-ai");
    assert_eq!(updated["settings"]["parallel_run_cap"], "4");
    assert!(updated["settings"].get("paused").is_none());
    assert!(service.path().exists());
    assert!(!refine_dir.join(SETTINGS_FILE).exists());
    let generic = service
        .update(&serde_json::json!({"agent_cli": "/opt/refine/custom-agent"}))
        .unwrap();
    assert_eq!(generic["settings"]["agent_cli"], "/opt/refine/custom-agent");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_settings_service_removes_retired_global_email_settings() {
    let temp_root = unique_temp_dir("settings-retired-email");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        serde_json::to_vec_pretty(&json!({
            "nodes": [{
                "id": "default",
                "display_name": "Default",
                "created_at": "2026-08-06T00:00:00Z",
                "updated_at": "2026-08-06T00:00:00Z",
                "settings": {
                    "agent_cli": "codex",
                    "development_request_email_enabled": "1",
                    "development_request_allowed_senders": "private@example.com"
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let settings = FileSettingsService::new(&refine_dir).load().unwrap();
    assert_eq!(settings["agent_cli"], "codex");
    assert!(!settings.contains_key("development_request_email_enabled"));
    let stored = fs::read_to_string(refine_dir.join("nodes.json")).unwrap();
    assert!(!stored.contains("development_request_email_enabled"));
    assert!(!stored.contains("development_request_allowed_senders"));
    assert!(!stored.contains("private@example.com"));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_settings_service_validates_generated_worktree_cleanup_paths() {
    let temp_root = unique_temp_dir("settings-worktree-cleanup-paths");
    let service = FileSettingsService::new(temp_root.join("state"));

    let updated = service
        .update(&json!({
            "worktree_cleanup_generated_paths": " build, .venv\nbuild "
        }))
        .unwrap();
    assert_eq!(
        updated["settings"]["worktree_cleanup_generated_paths"],
        ".venv, build"
    );
    assert!(
        service
            .update(&json!({"worktree_cleanup_generated_paths": "../outside"}))
            .is_err()
    );
    assert!(
        service
            .update(&json!({"worktree_cleanup_generated_paths": "/absolute"}))
            .is_err()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_settings_service_can_hold_one_resolved_non_default_node_identity() {
    let temp_root = unique_temp_dir("settings-fixed-node");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        serde_json::to_vec_pretty(&json!({
            "nodes": [
                {
                    "id": "default",
                    "display_name": "Default",
                    "created_at": "2026-07-22T00:00:00Z",
                    "updated_at": "2026-07-22T00:00:00Z",
                    "settings": {"agent_cli": "claude", "parallel_run_cap": "2"}
                },
                {
                    "id": "node-b",
                    "display_name": "Node B",
                    "created_at": "2026-07-22T00:00:00Z",
                    "updated_at": "2026-07-22T00:00:00Z",
                    "settings": {
                        "agent_cli": "smoke-ai",
                        "parallel_run_cap": "1",
                        "paused": "1",
                        "supervisor_agent_stall_seconds": "900"
                    }
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let settings = FileSettingsService::for_node(&refine_dir, "node-b")
        .load()
        .unwrap();
    assert_eq!(settings["agent_cli"], "smoke-ai");
    assert_eq!(settings["parallel_run_cap"], "1");
    assert!(settings.get("paused").is_none());
    assert!(settings.get(RETIRED_SUPERVISOR_STALL_KEY).is_none());
    let written = fs::read_to_string(refine_dir.join("nodes.json")).unwrap();
    assert!(!written.contains("\"paused\""));
    assert!(!written.contains(RETIRED_SUPERVISOR_STALL_KEY));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_settings_service_normalizes_node_stored_build_settings() {
    let temp_root = unique_temp_dir("settings-build-migration");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        serde_json::to_string_pretty(&json!({
            "nodes": [{
                "id": "default",
                "display_name": "Default",
                "created_at": "2026-06-16T00:00:00Z",
                "updated_at": "2026-06-16T00:00:00Z",
                "settings": {
                    "target_app_rebuild_command": "npm run build",
                    "target_app_rebuild_instructions": "Build and repair setup issues",
                    "target_app_rebuild_timeout_seconds": "45",
                    "target_app_auto_rebuild": "daily",
                    "target_app_auto_rebuild_hour_utc": "4",
                    "quality_timing": "post_rebuild"
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let service = FileSettingsService::new(&refine_dir);
    let settings = service.load().unwrap();
    assert_eq!(settings["target_app_build_command"], "npm run build");
    assert_eq!(
        settings["target_app_build_instructions"],
        "Build and repair setup issues"
    );
    assert_eq!(settings["target_app_build_timeout_seconds"], "45");
    assert_eq!(settings["target_app_auto_build"], "daily");
    assert_eq!(settings["target_app_auto_build_hour_utc"], "4");
    assert_eq!(settings["quality_timing"], "post_build");
    let written = fs::read_to_string(service.path()).unwrap();
    assert!(written.contains("target_app_build_command"));
    assert!(written.contains("target_app_build_instructions"));
    assert!(!written.contains("target_app_rebuild_command"));
    assert!(!written.contains("target_app_rebuild_instructions"));
    assert!(!refine_dir.join(SETTINGS_FILE).exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_settings_service_syncs_target_app_test_command_list() {
    let temp_root = unique_temp_dir("settings-test-commands");
    let refine_dir = temp_root.join(".refine");
    let service = FileSettingsService::new(&refine_dir);

    let legacy = service
        .update(&json!({
            "target_app_test_command": "npm test"
        }))
        .unwrap();
    assert_eq!(legacy["settings"]["target_app_test_command"], "npm test");
    assert_eq!(
        legacy["settings"]["target_app_test_commands"],
        r#"[{"command":"npm test","enabled":true}]"#
    );

    let updated = service
        .update(&json!({
            "target_app_test_commands": [
                {"command": "npm run lint", "enabled": false},
                {"command": "npm test", "enabled": true},
                {"command": "npm run e2e", "enabled": true}
            ]
        }))
        .unwrap();
    assert_eq!(updated["settings"]["target_app_test_command"], "npm test");
    assert_eq!(
        updated["settings"]["target_app_test_commands"],
        r#"[{"command":"npm run lint","enabled":false},{"command":"npm test","enabled":true},{"command":"npm run e2e","enabled":true}]"#
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_project_config_services_persist_governance_guidance_and_reporters() {
    let temp_root = unique_temp_dir("project-config");
    let refine_dir = temp_root.join(".refine");

    let governance = FileGovernanceService::new(&refine_dir);
    let saved = governance
        .save(&json!({
            "product": "Refine",
            "constitution": "Be useful",
            "rules": [{"text": "No regressions"}]
        }))
        .unwrap();
    assert_eq!(saved["configured"], true);
    assert_eq!(saved["rules"].as_array().unwrap().len(), 1);
    assert_eq!(
        governance
            .generate_rules(&json!({"product": "Refine", "constitution": "Be useful"}))
            .unwrap()["ok"],
        true
    );

    let guidance = FileGuidanceService::new(&refine_dir);
    let guidance_payload = guidance
        .update(&json!({"guidance": [{
            "name": "Accessibility",
            "rule": "When UI changes",
            "instructions": "Check keyboard behavior",
            "enabled": true
        }]}))
        .unwrap();
    assert_eq!(guidance_payload["guidance"].as_array().unwrap().len(), 1);

    let goal_dir = refine_dir.join("goals/GO/AL1");
    let feature_dir = refine_dir.join("features/FE/A1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::create_dir_all(&feature_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        serde_json::to_string_pretty(&json!({
            "id": "GOAL1",
            "reporter": "Buddy",
            "rounds": [
                {"reporter": "Alex", "assignee": "Buddy", "prompt": "B"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        feature_dir.join("feature.json"),
        serde_json::to_string_pretty(&json!({
            "id": "FEA1",
            "reporter": "Buddy",
            "assignee": "Buddy"
        }))
        .unwrap(),
    )
    .unwrap();

    let reporters = FileReporterService::new(&refine_dir);
    let buddy = reporters.create("Buddy").unwrap()["reporter"].clone();
    let alex = reporters.create("Alex").unwrap()["reporter"].clone();
    reporters
        .rename(buddy["id"].as_u64().unwrap(), "Buddy Williams")
        .unwrap();
    let merged = reporters
        .merge(buddy["id"].as_u64().unwrap(), alex["id"].as_u64().unwrap())
        .unwrap();
    assert_eq!(merged["ok"], true);
    assert_eq!(
        reporters.list().unwrap()["reporters"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let goal: Value =
        serde_json::from_str(&fs::read_to_string(goal_dir.join("goal.json")).unwrap()).unwrap();
    assert_eq!(goal["reporter"], "Alex");
    assert_eq!(goal["rounds"][0]["reporter"], "Alex");
    assert_eq!(goal["rounds"][0]["assignee"], "Alex");
    let feature: Value =
        serde_json::from_str(&fs::read_to_string(feature_dir.join("feature.json")).unwrap())
            .unwrap();
    assert_eq!(feature["reporter"], "Alex");
    assert_eq!(feature["assignee"], "Alex");

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

// Seeding a concurrency cap meant the scheduler never saw the key as unset, so
// the host-capacity governor it falls back to could not be reached through any
// supported configuration: a fixed 2 applied to a two-core node and a
// thirty-two-core one alike.
#[test]
fn concurrency_caps_are_unset_until_an_operator_chooses_one() {
    let temp_root = unique_temp_dir("settings-caps-unset");
    let refine_dir = temp_root.join(".refine");
    let service = FileSettingsService::new(&refine_dir);

    let defaults = service.load().unwrap();
    for key in [
        "parallel_run_cap",
        "parallel_per_node_cap",
        "parallel_per_provider_cap",
        "parallel_per_target_app_cap",
    ] {
        assert!(
            defaults.get(key).is_none(),
            "{key} must be absent so the host decides"
        );
    }

    // An explicit choice is still stored and still wins.
    let updated = service
        .update(&serde_json::json!({"parallel_run_cap": 4}))
        .unwrap();
    assert_eq!(updated["settings"]["parallel_run_cap"], "4");
    assert_eq!(service.load().unwrap()["parallel_run_cap"], "4");

    // And clearing it hands the decision back, which previously required
    // hand-editing the node registry because the range rejects every value the
    // scheduler reads as unset.
    let cleared = service
        .update(&serde_json::json!({"parallel_run_cap": ""}))
        .unwrap();
    assert_eq!(cleared["settings"]["parallel_run_cap"], "");
    assert!(
        service.load().unwrap()["parallel_run_cap"]
            .as_str()
            .is_none_or(str::is_empty)
    );

    // Out-of-range values are still rejected rather than silently accepted.
    assert!(
        service
            .update(&serde_json::json!({"parallel_run_cap": 0}))
            .is_err()
    );

    fs::remove_dir_all(temp_root).unwrap();
}
