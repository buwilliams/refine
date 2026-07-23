use crate::process::subprocess::{FileProcessSupervisor, ProcessSupervisor};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::WorkflowPolicy;
use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::service::SETTINGS_MIGRATION_VERSION;
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
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"summary\":\"Both checks passed.\",\"results\":[{\"test\":\"Dashboard loads\",\"status\":\"passed\",\"evidence\":\"Focused check planned\",\"command\":\"printf dashboard-ok\"},{\"test\":\"Keyboard navigation works\",\"status\":\"passed\",\"evidence\":\"Keyboard check planned\",\"command\":\"printf keyboard-ok\"}]}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let candidate_commit = init_git_candidate(&candidate_root);
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
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit: candidate_commit.clone(),
            process_metadata: quality_operation_metadata(&runtime_root),
        })
        .unwrap();
    assert!(result.ok, "{result:#?}");
    assert_eq!(
        result.summary,
        "All Quality tests passed with observed supervised evidence."
    );
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[0].test, "Dashboard loads");
    assert_eq!(result.results[0].command, "printf dashboard-ok");
    assert!(result.results[0].process_id.is_some());
    assert_eq!(result.results[0].exit_code, Some(0));

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

#[test]
fn quality_evaluation_rejects_an_extra_unobserved_pass_claim() {
    let result = parse_quality_provider_output(
        "GOAL1",
        &["Configured outcome".to_string()],
        r#"{"ok":true,"summary":"Done","results":[{"test":"Configured outcome","status":"passed","evidence":"Observed","command":"printf configured"},{"test":"Unconfigured browser claim","status":"passed","evidence":"Claimed only","command":"npm test"}]}"#,
    )
    .unwrap();

    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].status, "failed");
    assert!(result.results[0].evidence.contains("2 result(s)"));
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

    let service = FileQualityService::new(&refine_dir);
    let migrated = service.load_settings().unwrap();
    assert_eq!(migrated.timing, POST_BUILD);
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

    FileSettingsService::new(&refine_dir)
        .update(&json!({"quality_timing": "pre_merge"}))
        .unwrap();
    assert_eq!(service.load_settings().unwrap().timing, PRE_MERGE);
    assert_eq!(
        FileSettingsService::new(&refine_dir).load().unwrap()["quality_timing"],
        PRE_MERGE
    );

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

#[test]
fn quality_project_migration_retries_after_staged_failure() {
    let temp_root = unique_temp_dir("quality-migration-retry");
    let refine_dir = temp_root.join("state");
    fs::create_dir_all(&refine_dir).unwrap();
    let commands = serde_json::to_string(&json!([
        {"command": "printf inactive-check", "enabled": true}
    ]))
    .unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        serde_json::to_string_pretty(&json!({"nodes": [
            legacy_quality_node("default", "pre_merge", "[]"),
            legacy_quality_node("inactive", "pre_merge", &commands)
        ]}))
        .unwrap(),
    )
    .unwrap();

    let mut failing = FileQualityService::new(&refine_dir);
    failing.migration_failure_after_stage = true;
    let error = failing.load_settings().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected Quality migration failure")
    );
    let staged: Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join(SETTINGS_FILE)).unwrap()).unwrap();
    assert_eq!(staged["migration_version"], 0);
    assert_eq!(staged["legacy_commands"], json!(["printf inactive-check"]));

    let migrated = FileQualityService::new(&refine_dir)
        .load_settings()
        .unwrap();
    assert_eq!(migrated.legacy_commands, vec!["printf inactive-check"]);
    let completed: Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join(SETTINGS_FILE)).unwrap()).unwrap();
    assert_eq!(completed["migration_version"], SETTINGS_MIGRATION_VERSION);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_rejects_agent_pass_without_successful_observed_execution() {
    let temp_root = unique_temp_dir("quality-false-positive");
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Outcome works\",\"status\":\"passed\",\"evidence\":\"claimed\",\"command\":\"false\"}]}'\n",
    )
    .unwrap();
    make_executable(&smoke_ai);
    let candidate_commit = init_git_candidate(&candidate_root);
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai) };
    let service = FileQualityService::with_runtime_root(&refine_dir, &runtime_root);
    service
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Outcome works".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    let result = service
        .run_checks(QualityCheckRequest {
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit,
            process_metadata: quality_operation_metadata(&runtime_root),
        })
        .unwrap();
    assert!(!result.ok);
    assert_eq!(result.results[0].status, "failed");
    assert_eq!(result.results[0].exit_code, Some(1));
    assert!(result.results[0].process_id.is_some());
    restore_smoke_ai(previous);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_detects_candidate_mutation_and_preserves_it() {
    let temp_root = unique_temp_dir("quality-candidate-mutation");
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Candidate remains stable\",\"status\":\"passed\",\"evidence\":\"claimed\",\"command\":\"printf mutation >> candidate.txt\"}]}'\n",
    )
    .unwrap();
    make_executable(&smoke_ai);
    let candidate_commit = init_git_candidate(&candidate_root);
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai) };
    let service = FileQualityService::with_runtime_root(&refine_dir, &runtime_root);
    service
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Candidate remains stable".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    let error = service
        .run_checks(QualityCheckRequest {
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit,
            process_metadata: quality_operation_metadata(&runtime_root),
        })
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("dirty candidate index or worktree")
    );
    assert!(
        fs::read_to_string(candidate_root.join("candidate.txt"))
            .unwrap()
            .contains("mutation")
    );
    restore_smoke_ai(previous);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quality_operation_settles_parsing_failure_and_persists_the_same_goal_evidence() {
    let fixture = goal_quality_fixture("quality-parse-settlement", "printf 'not json\\n'");
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
    let error = fixture
        .runner()
        .run_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("required JSON evaluation"));
    let operation = FileOperationRegistry::new(&fixture.runtime_root)
        .recover()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(operation.state, OperationState::Failed);
    assert!(
        operation.error.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("required JSON evaluation")
    );
    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "failed");
    assert!(
        detail["rounds"][0]["quality_message"]
            .as_str()
            .unwrap()
            .contains("required JSON evaluation")
    );
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_operation_preserves_provider_failure_and_settles_terminally() {
    let fixture = goal_quality_fixture(
        "quality-provider-settlement",
        "printf 'AUTH TOKEN EXPIRED\\n' >&2; exit 17",
    );
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
    let error = fixture
        .runner()
        .run_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("AUTH TOKEN EXPIRED"));
    let operation = FileOperationRegistry::new(&fixture.runtime_root)
        .recover()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(operation.state, OperationState::Failed);
    assert!(
        operation.error.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("AUTH TOKEN EXPIRED")
    );
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn manual_quality_rejects_foreign_node_before_registering_an_operation() {
    let fixture = goal_quality_fixture("quality-manual-node-owner", "exit 99");
    fs::create_dir_all(&fixture.runtime_root).unwrap();
    fs::write(
        fixture.runtime_root.join("active-node.json"),
        serde_json::to_vec_pretty(&json!({"active_node_id": "node-b"})).unwrap(),
    )
    .unwrap();

    let error = fixture
        .runner()
        .start_manual_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("owned by node default"));
    assert!(error.to_string().contains("active node node-b"));
    assert!(
        FileOperationRegistry::new(&fixture.runtime_root)
            .recover()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn manual_quality_uses_shared_workflow_agent_capacity() {
    let fixture = goal_quality_fixture("quality-manual-capacity", "exit 99");
    let capacity = AgentCapacityService::new(&fixture.runtime_root);
    let policy = WorkflowPolicy {
        global_limit: 1,
        per_node_limit: 1,
        per_provider_limit: 1,
        per_target_app_limit: 1,
        active_node_id: "default".to_string(),
        provider: "smoke-ai".to_string(),
        target_app_id: fixture.candidate_root.display().to_string(),
    };
    assert!(
        capacity
            .try_acquire(
                &policy,
                AgentCapacityRequest {
                    owner_id: "workflow:existing".to_string(),
                    role: "workflow".to_string(),
                    node_id: "default".to_string(),
                    provider: "smoke-ai".to_string(),
                    target_app_id: fixture.candidate_root.display().to_string(),
                },
            )
            .unwrap()
    );
    FileSettingsService::with_active_root(&fixture.refine_dir, &fixture.runtime_root)
        .update(&json!({
            "parallel_run_cap": "1",
            "parallel_per_node_cap": "1",
            "parallel_per_provider_cap": "1",
            "parallel_per_target_app_cap": "1"
        }))
        .unwrap();

    let error = fixture
        .runner()
        .start_manual_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("concurrency limit"));
    assert!(
        FileOperationRegistry::new(&fixture.runtime_root)
            .recover()
            .unwrap()
            .is_empty()
    );
    capacity.release("workflow:existing").unwrap();
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_uses_one_non_default_node_for_result_and_error_persistence() {
    for (case, provider_body, expected_state) in [
        (
            "result",
            "printf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Outcome works\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"printf ok\"}]}'",
            "passed",
        ),
        ("error", "printf '%s\\n' 'not-json'", "failed"),
    ] {
        let fixture =
            goal_quality_fixture(&format!("quality-non-default-node-{case}"), provider_body);
        set_fixture_goal_node(&fixture, "node-b");
        let _guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };

        let runner = fixture.runner();
        let (operation, request) = runner
            .register_goal_checks("GOAL1", "smoke-ai", Default::default())
            .unwrap();
        assert_eq!(request.node_id, "node-b");
        assert_eq!(operation.request["node_id"], "node-b");
        // Runtime selection remains default; persistence must use the identity captured above.
        let _ = runner.run_registered(&operation.id, request);
        let detail = FileWorkItemService::new(&fixture.refine_dir)
            .show_goal_detail("GOAL1")
            .unwrap();
        assert_eq!(detail["rounds"][0]["quality_state"], expected_state);

        restore_smoke_ai(previous);
        fs::remove_dir_all(fixture.temp_root).unwrap();
    }
}

#[test]
fn quality_operation_is_exclusive_and_cancellation_terminates_its_provider() {
    let fixture = goal_quality_fixture("quality-cancel-exclusive", "exec sleep 30");
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
    let operation = fixture
        .runner()
        .start_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    assert_eq!(
        operation.request["target_root"],
        fixture.candidate_root.display().to_string()
    );
    assert_eq!(
        operation.request["refine_dir"],
        fixture.refine_dir.display().to_string()
    );
    let conflict = fixture
        .runner()
        .run_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap_err();
    assert!(conflict.to_string().contains(&operation.id));
    let process = wait_for_operation_process(&fixture.runtime_root, &operation.id);
    FileOperationRegistry::new(&fixture.runtime_root)
        .cancel_supervised(&operation.id, &|| Ok(()))
        .unwrap();
    wait_for_operation_state(
        &fixture.runtime_root,
        &operation.id,
        OperationState::Cancelled,
    );
    wait_for_process_exit(&process);
    FileProcessSupervisor::new(&fixture.runtime_root)
        .recover()
        .unwrap();
    wait_for_no_operation_process(&fixture.runtime_root, &operation.id);
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_cancellation_before_provider_launch_records_cancelled_evidence() {
    let fixture = goal_quality_fixture(
        "quality-cancel-before-provider",
        "printf launched > provider-launched; printf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Outcome works\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"printf ok\"}]}'",
    );
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
    let runner = fixture.runner();
    let (operation, request) = runner
        .register_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    FileOperationRegistry::new(&fixture.runtime_root)
        .cancel(&operation.id)
        .unwrap();
    let error = runner.run_registered(&operation.id, request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cancellation prevented provider launch")
    );
    assert!(!fixture.candidate_root.join("provider-launched").exists());
    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "cancelled");
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_cancellation_between_commands_prevents_later_work() {
    let fixture = goal_quality_fixture(
        "quality-cancel-between-commands",
        "printf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"First outcome\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"printf started > first-started; while [ ! -f release-first ]; do sleep 1; done\"},{\"test\":\"Second outcome\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"printf second > second-ran\"}]}'",
    );
    FileQualityService::new(&fixture.refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec![
                "First outcome".to_string(),
                "Second outcome".to_string(),
            ]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
    let operation = fixture
        .runner()
        .start_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    wait_for_path(&fixture.candidate_root.join("first-started"));
    FileOperationRegistry::new(&fixture.runtime_root)
        .cancel_supervised(&operation.id, &|| Ok(()))
        .unwrap();
    wait_for_operation_state(
        &fixture.runtime_root,
        &operation.id,
        OperationState::Cancelled,
    );
    wait_for_quality_state(&fixture.refine_dir, "cancelled");
    assert!(!fixture.candidate_root.join("second-ran").exists());
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_cancelled_evidence_failures_remain_nonterminal_and_restart_retries_them() {
    for failure in ["summary", "log"] {
        let fixture = goal_quality_fixture(
            &format!("quality-cancelled-{failure}-persistence"),
            "printf provider-must-not-launch > provider-launched",
        );
        let runner = fixture.runner();
        let (operation, request) = runner
            .register_goal_checks("GOAL1", "smoke-ai", Default::default())
            .unwrap();
        let blocked_path = if failure == "summary" {
            let summary = FileWorkItemService::new(&fixture.refine_dir)
                .show_goal_summary("GOAL1")
                .unwrap();
            fixture.refine_dir.join(summary.goal.json_path)
        } else {
            fixture.refine_dir.join("goals/GO/AL1/logs.jsonl")
        };
        let backup = blocked_path.with_extension("backup");
        if blocked_path.exists() {
            fs::rename(&blocked_path, &backup).unwrap();
        }
        fs::create_dir_all(&blocked_path).unwrap();

        let registry = FileOperationRegistry::new(&fixture.runtime_root);
        let cancelling = registry.cancel(&operation.id).unwrap();
        assert_eq!(cancelling.state, OperationState::Cancelling);
        let error = runner.run_registered(&operation.id, request).unwrap_err();
        assert!(error.to_string().contains(if failure == "summary" {
            "Goal"
        } else {
            "Goal log sidecar"
        }));
        assert_eq!(
            registry.status(&operation.id).unwrap().state,
            OperationState::Cancelling
        );
        assert!(!fixture.candidate_root.join("provider-launched").exists());

        let recovered = registry.recover_active_supervised().unwrap();
        assert!(
            recovered.iter().any(|item| {
                item.id == operation.id && item.state == OperationState::Cancelling
            })
        );
        runner.recover_cancelled_operations().unwrap_err();
        assert_eq!(
            registry.status(&operation.id).unwrap().state,
            OperationState::Cancelling
        );

        fs::remove_dir(&blocked_path).unwrap();
        if backup.exists() {
            fs::rename(&backup, &blocked_path).unwrap();
        }
        let settled = runner.recover_cancelled_operations().unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].id, operation.id);
        assert_eq!(settled[0].state, OperationState::Cancelled);
        assert!(runner.recover_cancelled_operations().unwrap().is_empty());

        let detail = FileWorkItemService::new(&fixture.refine_dir)
            .show_goal_detail("GOAL1")
            .unwrap();
        assert_eq!(detail["rounds"][0]["quality_state"], "cancelled");
        assert_eq!(
            detail["rounds"][0]["quality_details"]["operation_id"],
            operation.id
        );
        let cancelled_logs = FileLogService::new(&fixture.refine_dir)
            .all_round_logs("GOAL1")
            .unwrap()
            .into_iter()
            .filter(|entry| {
                entry.round_idx == Some(0)
                    && entry.entry.category == "quality"
                    && entry.entry.message == "Quality checks cancelled."
                    && entry
                        .entry
                        .details
                        .as_ref()
                        .and_then(|details| details.get("operation_id"))
                        == Some(&json!(operation.id))
            })
            .count();
        assert_eq!(cancelled_logs, 1);
        fs::remove_dir_all(fixture.temp_root).unwrap();
    }
}

#[test]
fn quality_evidence_persistence_failures_remain_nonterminal_and_recoverable() {
    for failure in ["summary", "log"] {
        let fixture = goal_quality_fixture(
            &format!("quality-{failure}-persistence"),
            "printf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Outcome works\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"printf ok\"}]}'",
        );
        let _guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
        let runner = fixture.runner();
        let (operation, request) = runner
            .register_goal_checks("GOAL1", "smoke-ai", Default::default())
            .unwrap();

        let blocked_path = if failure == "summary" {
            let summary = FileWorkItemService::new(&fixture.refine_dir)
                .show_goal_summary("GOAL1")
                .unwrap();
            fixture.refine_dir.join(summary.goal.json_path)
        } else {
            fixture.refine_dir.join("goals/GO/AL1/logs.jsonl")
        };
        let backup = blocked_path.with_extension("backup");
        if failure == "summary" {
            fs::rename(&blocked_path, &backup).unwrap();
        } else if blocked_path.exists() {
            fs::rename(&blocked_path, &backup).unwrap();
        }
        fs::create_dir_all(&blocked_path).unwrap();

        let error = runner.run_registered(&operation.id, request).unwrap_err();
        assert!(error.to_string().contains(if failure == "summary" {
            "Goal"
        } else {
            "Goal log sidecar"
        }));
        let registry = FileOperationRegistry::new(&fixture.runtime_root);
        assert_eq!(
            registry.status(&operation.id).unwrap().state,
            OperationState::Running
        );
        let logs = registry.page_logs(&operation.id, 50, 0).unwrap().0;
        assert!(
            logs.iter()
                .any(|entry| entry.message.contains("evidence persistence failed"))
        );

        fs::remove_dir(&blocked_path).unwrap();
        if backup.exists() {
            fs::rename(&backup, &blocked_path).unwrap();
        }
        let recovered = registry.recover_active_supervised().unwrap();
        assert!(
            recovered.iter().any(|item| {
                item.id == operation.id && item.state == OperationState::Interrupted
            })
        );
        restore_smoke_ai(previous);
        fs::remove_dir_all(fixture.temp_root).unwrap();
    }
}

#[test]
fn quality_operation_restart_recovery_interrupts_and_terminates_its_provider() {
    let fixture = goal_quality_fixture("quality-restart-recovery", "exec sleep 30");
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };
    let operation = fixture
        .runner()
        .start_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    wait_for_operation_process(&fixture.runtime_root, &operation.id);
    let recovered = FileOperationRegistry::new(&fixture.runtime_root)
        .recover_active_supervised()
        .unwrap();
    FileProcessSupervisor::new(&fixture.runtime_root)
        .recover()
        .unwrap();
    assert!(
        recovered
            .iter()
            .any(|item| item.id == operation.id && item.state == OperationState::Interrupted)
    );
    assert!(
        FileProcessSupervisor::new(&fixture.runtime_root)
            .list()
            .unwrap()
            .iter()
            .all(|process| !process
                .details
                .as_deref()
                .unwrap_or("")
                .contains(&operation.id))
    );
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn quality_operation_metadata(runtime_root: &PathBuf) -> serde_json::Map<String, Value> {
    let operation = FileOperationRegistry::new(runtime_root)
        .register("quality:test")
        .unwrap();
    serde_json::Map::from_iter([("operation_id".to_string(), json!(operation.id))])
}

fn legacy_quality_node(id: &str, timing: &str, commands: &str) -> Value {
    json!({
        "id": id,
        "display_name": id,
        "created_at": "2026-07-22T00:00:00Z",
        "updated_at": "2026-07-22T00:00:00Z",
        "settings": {
            "quality_enabled": "1",
            "quality_timing": timing,
            "target_app_test_commands": commands
        }
    })
}

fn init_git_candidate(root: &PathBuf) -> String {
    fs::create_dir_all(root).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "quality@example.com"],
        vec!["config", "user.name", "Quality Test"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(root.join("candidate.txt"), "candidate\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "."])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-m", "candidate"])
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn make_executable(path: &PathBuf) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn restore_smoke_ai(previous: Option<std::ffi::OsString>) {
    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
}

struct GoalQualityFixture {
    temp_root: PathBuf,
    candidate_root: PathBuf,
    refine_dir: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
}

impl GoalQualityFixture {
    fn runner(&self) -> QualityOperationRunner {
        QualityOperationRunner::new(&self.refine_dir, &self.runtime_root, &self.candidate_root)
    }
}

fn goal_quality_fixture(prefix: &str, provider_body: &str) -> GoalQualityFixture {
    let temp_root = unique_temp_dir(prefix);
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(&smoke_ai, format!("#!/bin/sh\n{provider_body}\n")).unwrap();
    make_executable(&smoke_ai);
    let candidate_commit = init_git_candidate(&candidate_root);
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&candidate_root)
            .args(["branch", "-m", "refine/GOAL1/round-1"])
            .status()
            .unwrap()
            .success()
    );
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Quality candidate", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "test", "Verify candidate")
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &candidate_commit,
            Some(&candidate_commit),
        )
        .unwrap();
    FileQualityService::new(&refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Outcome works".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    GoalQualityFixture {
        temp_root,
        candidate_root,
        refine_dir,
        runtime_root,
        smoke_ai,
    }
}

fn set_fixture_goal_node(fixture: &GoalQualityFixture, node_id: &str) {
    let summary = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_summary("GOAL1")
        .unwrap();
    let path = fixture.refine_dir.join(summary.goal.json_path);
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["node_id"] = json!(node_id);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn wait_for_operation_process(
    runtime_root: &PathBuf,
    operation_id: &str,
) -> crate::process::subprocess::ManagedProcess {
    for _ in 0..200 {
        if let Some(process) = FileProcessSupervisor::new(runtime_root)
            .list()
            .unwrap()
            .into_iter()
            .find(|process| {
                process
                    .details
                    .as_deref()
                    .unwrap_or("")
                    .contains(operation_id)
            })
        {
            return process;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("managed process for operation {operation_id} was not observed");
}

fn wait_for_path(path: &PathBuf) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("path {} was not created", path.display());
}

fn wait_for_quality_state(refine_dir: &PathBuf, expected: &str) {
    for _ in 0..200 {
        if FileWorkItemService::new(refine_dir)
            .show_goal_detail("GOAL1")
            .ok()
            .and_then(|detail| {
                detail["rounds"][0]["quality_state"]
                    .as_str()
                    .map(str::to_string)
            })
            .as_deref()
            == Some(expected)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("Goal Quality state did not reach {expected}");
}

fn wait_for_process_exit(process: &crate::process::subprocess::ManagedProcess) {
    for _ in 0..200 {
        if !FileProcessSupervisor::process_is_alive(process).unwrap() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("managed process {} did not exit", process.id);
}

fn wait_for_operation_state(runtime_root: &PathBuf, operation_id: &str, expected: OperationState) {
    for _ in 0..200 {
        let state = FileOperationRegistry::new(runtime_root)
            .status(operation_id)
            .unwrap()
            .state;
        if state == expected {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("operation {operation_id} did not reach {expected:?}");
}

fn wait_for_no_operation_process(runtime_root: &PathBuf, operation_id: &str) {
    for _ in 0..200 {
        if FileProcessSupervisor::new(runtime_root)
            .list()
            .unwrap()
            .iter()
            .all(|process| {
                !process
                    .details
                    .as_deref()
                    .unwrap_or("")
                    .contains(operation_id)
            })
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("managed process for operation {operation_id} did not exit");
}
