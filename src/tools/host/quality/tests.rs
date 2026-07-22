use crate::tools::host::agent_providers::smoke_ai_env_lock;
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
            tests: Some(vec!["Dashboard loads".to_string()]),
            enabled: Some(json!("1")),
            timing: Some(POST_BUILD.to_string()),
        })
        .unwrap();

    assert_eq!(saved.enabled, "1");
    assert_eq!(saved.timing, POST_BUILD);
    assert!(saved.configured);
    assert_eq!(saved.tests, vec!["Dashboard loads"]);
    assert!(refine_dir.join(SETTINGS_FILE).exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_service_uses_agent_to_evaluate_every_plain_text_test() {
    let temp_root = unique_temp_dir("quality-trait");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"summary\":\"Both checks passed.\",\"results\":[{\"test\":\"Dashboard loads\",\"status\":\"passed\",\"evidence\":\"Focused browser check passed\",\"command\":\"npm test\"},{\"test\":\"Keyboard navigation works\",\"status\":\"passed\",\"evidence\":\"Tab order inspected\",\"command\":\"\"}]}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai) };
    let service = FileQualityService::with_runtime_root(&refine_dir, &runtime_root);
    service
        .save_settings(QualitySettingsPatch {
            tests: Some(vec![
                " Dashboard   loads ".to_string(),
                "Keyboard navigation works".to_string(),
                "Dashboard loads".to_string(),
            ]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();

    let result = service
        .run_checks(QualityCheckRequest {
            owner_id: "GOAL1".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: temp_root.display().to_string(),
            process_metadata: Default::default(),
        })
        .unwrap();
    assert!(result.ok, "{result:#?}");
    assert_eq!(result.summary, "Both checks passed.");
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[0].test, "Dashboard loads");
    assert_eq!(result.results[0].command, "npm test");

    let gate = service.gate("GOAL1").unwrap();
    assert!(gate.ok);
    assert!(gate.diagnostics[0].contains("2 plain-text test(s)"));
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

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

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

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
