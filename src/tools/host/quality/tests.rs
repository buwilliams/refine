use crate::process::supervisor::config::FileSettingsService;
use crate::process::supervisor::errors::RefineError;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
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
            regressions_enabled: Some(json!(true)),
        })
        .unwrap();

    assert_eq!(saved.enabled, "1");
    assert_eq!(saved.timing, POST_BUILD);
    assert_eq!(saved.regressions_enabled, "1");
    assert!(saved.configured);
    assert!(refine_dir.join(SETTINGS_FILE).exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_regressions_create_update_delete_and_run() {
    let temp_root = unique_temp_dir("quality-regressions");
    let refine_dir = temp_root.join(".refine");
    write_fake_playwright(&temp_root, 0);
    FileSettingsService::new(&refine_dir)
        .update(&json!({"target_app_url": "http://127.0.0.1:3000"}))
        .unwrap();
    let service = FileQualityService::new(&refine_dir);
    service
        .save_settings(QualitySettingsPatch {
            regressions_enabled: Some(json!("1")),
            ..QualitySettingsPatch::default()
        })
        .unwrap();

    let created = service
        .create_regression("Dashboard smoke", "Open dashboard", "Navigate home")
        .unwrap();
    assert_eq!(created.id, "dashboard-smoke");
    assert!(
        refine_dir
            .join("regressions/specs/dashboard-smoke.spec.cjs")
            .exists()
    );

    let updated = service
        .update_regression(&created.id, &json!({"enabled": false}))
        .unwrap();
    assert!(!updated.enabled);
    let no_runs = service.run_regressions(true).unwrap();
    assert_eq!(no_runs.message, "No regression checks configured.");

    service
        .update_regression(&created.id, &json!({"enabled": true}))
        .unwrap();
    let result = service.run_regressions(true).unwrap();
    assert!(result.ok);
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.runs[0].message, "Playwright regression passed");
    assert!(result.runs[0].json_report_path.is_some());
    assert!(result.runs[0].screenshot_path.ends_with("screenshot.png"));

    let loaded = service.load_settings().unwrap();
    assert_eq!(loaded.regressions[0].latest_run.as_ref().unwrap().ok, true);

    service.delete_regression(&created.id).unwrap();
    assert!(service.list_regressions(true).unwrap().is_empty());
    assert!(
        !refine_dir
            .join("regressions/specs/dashboard-smoke.spec.cjs")
            .exists()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_regression_run_reports_missing_target_url_as_infra() {
    let temp_root = unique_temp_dir("quality-regression-infra");
    let refine_dir = temp_root.join(".refine");
    write_fake_playwright(&temp_root, 0);
    let service = FileQualityService::new(&refine_dir);
    service
        .save_settings(QualitySettingsPatch {
            regressions_enabled: Some(json!(true)),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    service
        .create_regression("Dashboard smoke", "Open dashboard", "Navigate home")
        .unwrap();

    let result = service.run_regressions(true).unwrap();
    assert!(!result.ok);
    assert!(result.infra);
    assert!(result.runs[0].message.contains("target app URL"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_service_runs_commands_compares_screenshots_and_gates() {
    let temp_root = unique_temp_dir("quality-trait");
    let refine_dir = temp_root.join(".refine");
    write_fake_playwright(&temp_root, 0);
    FileSettingsService::new(&refine_dir)
        .update(&json!({"target_app_url": "http://127.0.0.1:3000"}))
        .unwrap();
    let service = FileQualityService::new(&refine_dir);

    let command_result = service
        .run_checks(QualityCheckRequest {
            owner_id: "GAP1".to_string(),
            command: "printf command-ok".to_string(),
            browser_required: false,
            process_metadata: Default::default(),
        })
        .unwrap();
    assert!(command_result.ok);
    assert_eq!(command_result.diagnostics, vec!["command-ok"]);

    service
        .save_settings(QualitySettingsPatch {
            enabled: Some(json!(true)),
            regressions_enabled: Some(json!(true)),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    service
        .create_regression("Dashboard smoke", "Open dashboard", "Navigate home")
        .unwrap();
    let gate = service.gate("GAP1").unwrap();
    assert!(gate.ok);
    let screenshots = service.screenshots("GAP1").unwrap();
    assert_eq!(screenshots.len(), 1);

    let baseline = temp_root.join("baseline.png");
    let candidate = temp_root.join("candidate.png");
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
        owner_id: "GAP1".to_string(),
        command: "rm -rf target".to_string(),
        browser_required: false,
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

fn write_fake_playwright(root: &Path, exit_code: i32) {
    let bin_dir = root.join("node_modules/.bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join("playwright");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' '{{\"status\":\"passed\"}}'\nif [ -n \"$REFINE_REGRESSION_SCREENSHOT\" ]; then printf 'png' > \"$REFINE_REGRESSION_SCREENSHOT\"; fi\nexit {exit_code}"
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
}
