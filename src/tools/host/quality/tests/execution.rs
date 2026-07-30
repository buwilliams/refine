use super::*;

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
            fixture.refine_dir.join("runtime/goals/GO/AL1/logs.jsonl")
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
