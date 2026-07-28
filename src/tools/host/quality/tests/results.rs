use super::*;

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
