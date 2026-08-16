use super::*;
use crate::process::supervisor::errors::RefineError;

fn failed_quality_result(
    results: Vec<QualityTestResult>,
    diagnostics: Vec<String>,
) -> QualityCheckResult {
    QualityCheckResult {
        owner_id: "GOAL1".to_string(),
        ok: false,
        summary: String::new(),
        results,
        diagnostics,
        candidate_commit: "candidate".to_string(),
        checked_at: None,
        provider_attempts: Vec::new(),
    }
}

fn failed_test(
    test: &str,
    evidence: &str,
    command: &str,
    process_id: Option<&str>,
    exit_code: Option<i32>,
) -> QualityTestResult {
    QualityTestResult {
        test: test.to_string(),
        status: "failed".to_string(),
        evidence: evidence.to_string(),
        command: command.to_string(),
        process_id: process_id.map(str::to_string),
        exit_code,
    }
}

#[test]
fn quality_failure_summary_names_one_failed_test_and_observed_cause() {
    let result = failed_quality_result(
        vec![failed_test(
            "Dashboard loads",
            "Observed supervised process quality-1 exited 1. stderr: assertion failed",
            "cargo test dashboard",
            Some("quality-1"),
            Some(1),
        )],
        vec!["complete diagnostic retained in Details".to_string()],
    );

    assert_eq!(
        quality_failure_summary(&result),
        "Quality failed: “Dashboard loads” — supervised command exited with code 1."
    );
}

#[test]
fn quality_failure_summary_selects_high_signal_failure_and_counts_the_rest() {
    let result = failed_quality_result(
        vec![
            failed_test("First check", "agent reported failure", "", None, None),
            failed_test(
                "Compile check",
                "compiler output",
                "cargo check",
                Some("quality-2"),
                Some(101),
            ),
            failed_test("Third check", "missing evidence", "", None, None),
        ],
        Vec::new(),
    );

    assert_eq!(
        quality_failure_summary(&result),
        "Quality failed: “Compile check” — supervised command exited with code 101. 2 additional tests failed."
    );
}

#[test]
fn quality_failure_summary_has_clear_fallback_without_structured_evidence() {
    let result = failed_quality_result(Vec::new(), Vec::new());

    assert_eq!(
        quality_failure_summary(&result),
        "Quality failed: no valid structured failure evidence was recorded; inspect Details and supervised logs."
    );
}

#[test]
fn quality_failure_summary_explains_rejected_pass_without_supervised_evidence() {
    let result = failed_quality_result(
        vec![failed_test(
            "Release artifact is valid",
            "Pass claim rejected because no supervised command execution was requested.",
            "",
            None,
            None,
        )],
        Vec::new(),
    );

    assert_eq!(
        quality_failure_summary(&result),
        "Quality failed: “Release artifact is valid” — Pass claim rejected because no supervised command execution was requested."
    );
}

#[test]
fn quality_failure_summary_bounds_normalizes_and_redacts_diagnostics() {
    let result = failed_quality_result(
        Vec::new(),
        vec![format!(
            "Multiline diagnostic\n\twith token=do-not-repeat and {}",
            "x".repeat(600)
        )],
    );

    let summary = quality_failure_summary(&result);
    assert!(summary.starts_with("Quality failed: Multiline diagnostic with token=[redacted] and "));
    assert!(!summary.contains('\n'));
    assert!(!summary.contains("do-not-repeat"));
    assert!(summary.ends_with('…'));
    assert!(summary.chars().count() <= 400);
}

#[test]
fn quality_error_summary_preserves_bounded_harness_fault_cause() {
    let error = RefineError::Degraded(
        "Quality command harness fault: supervised shell could not parse the command\nsyntax error"
            .to_string(),
    );

    let summary = quality_error_summary(&error);
    assert!(summary.contains("Quality command harness fault"));
    assert!(summary.contains("syntax error"));
    assert!(!summary.contains('\n'));
    assert!(summary.chars().count() <= 400);
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
            stall_timeout_seconds: None,
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit: candidate_commit.clone(),
            identity_commitment: None,
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
            stall_timeout_seconds: None,
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit,
            identity_commitment: None,
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

#[cfg(unix)]
#[test]
fn quality_runs_supervised_commands_with_bash_process_substitution() {
    let temp_root = unique_temp_dir("quality-bash-process-substitution");
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Bash syntax works\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"test -r <(printf ok)\"}]}'\n",
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
            tests: Some(vec!["Bash syntax works".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();

    let result = service
        .run_checks(QualityCheckRequest {
            stall_timeout_seconds: None,
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit,
            identity_commitment: None,
            process_metadata: quality_operation_metadata(&runtime_root),
        })
        .unwrap();

    assert!(result.ok, "{result:#?}");
    assert_eq!(result.results[0].exit_code, Some(0));
    restore_smoke_ai(previous);
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn quality_records_shell_parser_aborts_as_harness_faults() {
    let fixture = goal_quality_fixture(
        "quality-shell-parser-harness-fault",
        "printf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"Outcome works\",\"status\":\"passed\",\"evidence\":\"planned\",\"command\":\"printf ok < <(\"}]}'",
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

    let error = runner.run_registered(&operation.id, request).unwrap_err();

    assert!(error.to_string().contains("Quality command harness fault"));
    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "harness_fault");
    assert_eq!(
        detail["rounds"][0]["quality_details"]["error_kind"],
        "harness_fault"
    );
    let settled = FileOperationRegistry::new(&fixture.runtime_root)
        .status(&operation.id)
        .unwrap();
    assert_eq!(settled.state, OperationState::Failed);
    assert_eq!(
        settled.error.as_ref().unwrap()["code"],
        "quality_command_harness_fault"
    );

    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_accepts_no_match_evidence_when_command_encodes_pass_semantics() {
    let temp_root = unique_temp_dir("quality-no-match-pass");
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"results\":[{\"test\":\"No forbidden warning is emitted\",\"status\":\"passed\",\"evidence\":\"no match is the passing predicate\",\"command\":\"! grep -q forbidden candidate.txt\"}]}'\n",
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
            tests: Some(vec!["No forbidden warning is emitted".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();

    let result = service
        .run_checks(QualityCheckRequest {
            stall_timeout_seconds: None,
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit,
            identity_commitment: None,
            process_metadata: quality_operation_metadata(&runtime_root),
        })
        .unwrap();

    assert!(result.ok, "{result:#?}");
    assert_eq!(result.results[0].status, "passed");
    assert_eq!(result.results[0].exit_code, Some(0));
    assert!(
        result.results[0]
            .evidence
            .contains("no match is the passing predicate")
    );
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
            stall_timeout_seconds: None,
            owner_id: "GOAL1".to_string(),
            round_idx: 0,
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            cwd: candidate_root.display().to_string(),
            source_candidate_commit: Some(candidate_commit.clone()),
            evaluation_scope: "isolated_candidate".to_string(),
            candidate_commit,
            identity_commitment: None,
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
fn manual_quality_observes_live_agent_process_capacity() {
    let fixture = goal_quality_fixture("quality-manual-capacity", "exit 99");
    let processes = fixture.runtime_root.join("processes");
    fs::create_dir_all(&processes).unwrap();
    fs::write(
        processes.join("existing-agent.json"),
        serde_json::to_vec_pretty(&ManagedProcess {
            id: "existing-agent".to_string(),
            owner: ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Existing Goal agent".to_string()),
            details: Some(
                json!({
                    "kind": "workflow",
                    "goal_id": "EXISTING",
                    "node_id": "default",
                    "provider": "smoke-ai",
                    "target_app_id": fixture.candidate_root.display().to_string()
                })
                .to_string(),
            ),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            exit_code: None,
        })
        .unwrap(),
    )
    .unwrap();
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
        (
            "error",
            "printf '%s\\n' 'not-json'",
            "output_contract_fault",
        ),
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
            fixture.refine_dir.join("runtime/goals/GO/AL1/logs.jsonl")
        };
        let backup = blocked_path.with_extension("backup");
        if failure == "summary" || blocked_path.exists() {
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
