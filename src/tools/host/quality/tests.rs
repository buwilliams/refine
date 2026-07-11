use crate::process::supervisor::config::FileSettingsService;
use crate::process::supervisor::errors::RefineError;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
            enabled: Some(json!("1")),
            timing: Some(POST_BUILD.to_string()),
        })
        .unwrap();

    assert_eq!(saved.enabled, "1");
    assert_eq!(saved.timing, POST_BUILD);
    assert!(saved.configured);
    assert!(refine_dir.join(SETTINGS_FILE).exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_service_runs_commands_compares_artifacts_and_gates() {
    let temp_root = unique_temp_dir("quality-trait");
    let refine_dir = temp_root.join(".refine");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"allowed_commands": "printf"}))
        .unwrap();
    let service = FileQualityService::new(&refine_dir);

    let command_result = service
        .run_checks(QualityCheckRequest {
            owner_id: "GOAL1".to_string(),
            command: "printf command-ok".to_string(),
            process_metadata: Default::default(),
        })
        .unwrap();
    assert!(command_result.ok);
    assert_eq!(command_result.diagnostics, vec!["command-ok"]);

    service
        .save_settings(QualitySettingsPatch {
            enabled: Some(json!(true)),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    let gate = service.gate("GOAL1").unwrap();
    assert!(gate.ok);
    assert!(gate.diagnostics[0].contains("target-app test command"));
    assert!(service.screenshots("GOAL1").unwrap().is_empty());

    let baseline = temp_root.join("baseline.txt");
    let candidate = temp_root.join("candidate.txt");
    fs::write(&baseline, b"same").unwrap();
    fs::write(&candidate, b"same").unwrap();
    assert!(
        service
            .compare(baseline.to_str().unwrap(), candidate.to_str().unwrap())
            .unwrap()
            .ok
    );
    fs::write(&candidate, b"different").unwrap();
    assert!(
        !service
            .compare(baseline.to_str().unwrap(), candidate.to_str().unwrap())
            .unwrap()
            .ok
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_service_enforces_allowed_commands_for_direct_checks() {
    let temp_root = unique_temp_dir("quality-security");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"allowed_commands": "printf"}))
        .unwrap();
    let service = FileQualityService::with_runtime_root(&refine_dir, &runtime_root);

    let denied = service.run_checks(QualityCheckRequest {
        owner_id: "GOAL1".to_string(),
        command: "rm -rf target".to_string(),
        process_metadata: Default::default(),
    });

    assert!(matches!(denied, Err(RefineError::Unauthorized(_))));
    let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
    assert!(audit.contains("\"outcome\":\"denied\""));

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
