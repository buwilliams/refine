use super::*;
use crate::model::workflow::GoalStatus;

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
    assert!(
        error
            .to_string()
            .contains("invalid structured Quality evaluation JSON"),
        "error: {error}"
    );
    let operation = FileOperationRegistry::new(&fixture.runtime_root)
        .recover()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(operation.state, OperationState::Failed);
    let (logs, _, _) = FileOperationRegistry::new(&fixture.runtime_root)
        .page_logs(&operation.id, 50, 0)
        .unwrap();
    let attempts = logs
        .iter()
        .filter_map(|entry| {
            entry
                .details
                .as_ref()
                .and_then(|details| details.get("provider_attempt"))
        })
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 3);
    assert!(
        attempts
            .iter()
            .all(|attempt| { attempt["raw_output"] == "not json" && attempt["accepted"] == false })
    );
    assert!(
        operation.error.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("invalid structured Quality evaluation JSON")
    );
    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    assert_eq!(
        detail["rounds"][0]["quality_state"],
        "output_contract_fault"
    );
    assert!(
        detail["rounds"][0]["quality_message"]
            .as_str()
            .unwrap()
            .contains("invalid structured Quality evaluation JSON")
    );
    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_repairs_one_invalid_response_and_retains_both_attempts() {
    let fixture = goal_quality_fixture(
        "quality-output-repair",
        "if test ! -e \"$0.attempted\"; then : > \"$0.attempted\"; printf 'not json\\n'; else printf '%s\\n' '{\"summary\":\"repaired\",\"results\":[{\"test\":\"Outcome works\",\"status\":\"passed\",\"evidence\":\"repaired plan\",\"command\":\"printf ok\"}]}'; fi",
    );
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };

    let operation = fixture
        .runner()
        .run_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    assert!(operation.result.ok, "{:#?}", operation.result);
    assert_eq!(operation.result.provider_attempts.len(), 2);
    assert!(!operation.result.provider_attempts[0].accepted);
    assert_eq!(operation.result.provider_attempts[0].raw_output, "not json");
    assert!(operation.result.provider_attempts[1].accepted);

    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    let quality = &detail["rounds"][0]["quality_details"];
    assert_eq!(quality["provider_attempts"].as_array().unwrap().len(), 2);
    assert_eq!(quality["quality_proof"]["goal_id"], "GOAL1");
    assert_eq!(quality["quality_proof"]["round_idx"], 0);
    assert_eq!(quality["quality_proof"]["state"], "passed");

    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn quality_cancellation_during_repair_prevents_further_attempts() {
    let fixture = goal_quality_fixture(
        "quality-output-repair-cancel",
        "if test ! -e \"$0.attempted\"; then : > \"$0.attempted\"; printf 'not json\\n'; else exec sleep 30; fi",
    );
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &fixture.smoke_ai) };

    let operation = fixture
        .runner()
        .start_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    let registry = FileOperationRegistry::new(&fixture.runtime_root);
    for _ in 0..100 {
        let (logs, _, _) = registry.page_logs(&operation.id, 50, 0).unwrap();
        if logs.iter().any(|entry| {
            entry
                .details
                .as_ref()
                .and_then(|details| details.get("provider_attempt"))
                .is_some()
        }) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    wait_for_operation_process(&fixture.runtime_root, &operation.id);
    registry
        .cancel_supervised(&operation.id, &|| Ok(()))
        .unwrap();
    wait_for_operation_state(
        &fixture.runtime_root,
        &operation.id,
        OperationState::Cancelled,
    );
    let (logs, _, _) = registry.page_logs(&operation.id, 50, 0).unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| {
                entry
                    .details
                    .as_ref()
                    .and_then(|details| details.get("provider_attempt"))
                    .is_some()
            })
            .count(),
        1
    );
    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "cancelled");

    restore_smoke_ai(previous);
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn workflow_quality_authority_loss_prevents_provider_launch() {
    let fixture = goal_quality_fixture(
        "quality-workflow-authority-fence",
        "printf launched > provider-launched; exit 99",
    );
    let work_items = FileWorkItemService::new(&fixture.refine_dir);
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    let (round_idx, workflow_revision, authored_request) =
        work_items.authored_goal_commitment("GOAL1").unwrap();
    work_items
        .claim_workflow_attempt(
            "GOAL1",
            GoalStatus::Quality,
            round_idx,
            workflow_revision,
            &authored_request,
        )
        .unwrap();
    let mut metadata = crate::infrastructure::process::subprocess::workflow_subprocess_metadata(
        "GOAL1",
        "quality",
        "WorkflowQuality",
        Some(round_idx),
    );
    metadata.insert("workflow_revision".to_string(), json!(workflow_revision));
    let runner = fixture.runner();
    let (operation, request) = runner
        .register_goal_checks("GOAL1", "smoke-ai", metadata)
        .unwrap();
    work_items.cancel_goal_summary("GOAL1").unwrap();

    let error = runner.run_registered(&operation.id, request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no longer authorizes quality work")
    );
    assert!(!fixture.candidate_root.join("provider-launched").exists());

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
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
        &fixture.refine_dir,
        &fixture.runtime_root,
    );
    nodes.create("node-b").unwrap();
    nodes.activate("node-b").unwrap();

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

#[test]
fn isolated_candidate_identity_faults_remain_unclassified_and_preserve_evidence() {
    for mutation in ["round", "branch", "dirty", "commit"] {
        let fixture = goal_quality_fixture(
            &format!("quality-identity-{mutation}"),
            "printf provider-must-not-launch > provider-launched",
        );
        let runner = fixture.runner();
        let (operation, request) = runner
            .register_goal_checks("GOAL1", "smoke-ai", Default::default())
            .unwrap();
        let expected_commit = request.candidate_commit.clone();
        match mutation {
            "round" => {
                FileWorkItemService::new(&fixture.refine_dir)
                    .append_goal_round_summary("GOAL1", "Reporter", "Changed authority")
                    .unwrap();
            }
            "branch" => {
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(&fixture.candidate_root)
                        .args(["branch", "-m", "wrong-branch"])
                        .status()
                        .unwrap()
                        .success()
                );
            }
            "dirty" => {
                fs::write(fixture.candidate_root.join("untracked.txt"), "preserve\n").unwrap();
            }
            "commit" => {
                fs::write(fixture.candidate_root.join("candidate.txt"), "advanced\n").unwrap();
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(&fixture.candidate_root)
                        .args(["add", "candidate.txt"])
                        .status()
                        .unwrap()
                        .success()
                );
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(&fixture.candidate_root)
                        .args(["commit", "-m", "advanced"])
                        .status()
                        .unwrap()
                        .success()
                );
            }
            _ => unreachable!(),
        }

        let error = runner.run_registered(&operation.id, request).unwrap_err();
        assert!(
            matches!(error, RefineError::QualityCandidateInfrastructure(_)),
            "{mutation}: {error}"
        );
        let settled = FileOperationRegistry::new(&fixture.runtime_root)
            .status(&operation.id)
            .unwrap();
        assert_eq!(settled.state, OperationState::Failed);
        assert_eq!(
            settled.error.as_ref().unwrap()["code"],
            "quality_candidate_infrastructure_fault"
        );
        let detail = FileWorkItemService::new(&fixture.refine_dir)
            .show_goal_detail("GOAL1")
            .unwrap();
        assert_eq!(detail["rounds"][0]["quality_state"], "unclassified");
        assert_eq!(detail["rounds"][0]["quality_details"], "");
        assert!(!fixture.candidate_root.join("provider-launched").exists());
        assert!(!expected_commit.is_empty());
        fs::remove_dir_all(fixture.temp_root).unwrap();
    }
}

#[test]
fn registered_candidate_deletion_is_infrastructure_failure_and_preserves_the_ref() {
    let temp_root = unique_temp_dir("quality-linked-candidate-deletion");
    let target_root = temp_root.join("target");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&target_root).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "quality@example.com"],
        vec!["config", "user.name", "Quality Test"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&target_root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args(["add", "app.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args(["commit", "-m", "base"])
            .status()
            .unwrap()
            .success()
    );
    let branch = "refine/GOAL1/round-1";
    let candidate_root = target_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(candidate_root.parent().unwrap()).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                candidate_root.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );
    let candidate = git_output(&candidate_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let refine_dir =
        crate::infrastructure::storage::project_layout::refine_dir_for_target_root(&target_root)
            .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Quality candidate", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Verify candidate")
        .unwrap();
    work_items
        .update_goal_git_refs("GOAL1", branch, "main", &candidate, Some(&candidate))
        .unwrap();
    FileQualityService::new(&refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Outcome works".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    let runner = QualityOperationRunner::new(&refine_dir, &runtime_root, &target_root);
    let (operation, request) = runner
        .register_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args([
                "worktree",
                "remove",
                "--force",
                candidate_root.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );

    let error = runner.run_registered(&operation.id, request).unwrap_err();
    assert!(matches!(
        error,
        RefineError::QualityCandidateInfrastructure(_)
    ));
    let settled = FileOperationRegistry::new(&runtime_root)
        .status(&operation.id)
        .unwrap();
    assert_eq!(
        settled.error.as_ref().unwrap()["code"],
        "quality_candidate_infrastructure_fault"
    );
    assert_eq!(
        git_output(&target_root, &["rev-parse", branch]).trim(),
        candidate
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "unclassified");
    assert_eq!(detail["rounds"][0]["quality_details"], "");
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn candidate_deletion_before_quality_registration_creates_no_quality_result() {
    let fixture = linked_goal_quality_fixture("quality-preflight-candidate-deletion");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.target_root)
            .args([
                "worktree",
                "remove",
                "--force",
                fixture.candidate_root.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );
    let runner = QualityOperationRunner::new(
        &fixture.refine_dir,
        &fixture.runtime_root,
        &fixture.target_root,
    );
    let error = runner
        .run_goal_checks("GOAL1", "smoke-ai", Default::default())
        .unwrap_err();
    assert!(matches!(
        error,
        RefineError::QualityCandidateInfrastructure(_)
    ));
    assert!(
        FileOperationRegistry::new(&fixture.runtime_root)
            .recover()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        git_output(&fixture.target_root, &["rev-parse", &fixture.branch]).trim(),
        fixture.candidate
    );
    let detail = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_detail("GOAL1")
        .unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "unclassified");
    assert_eq!(detail["rounds"][0]["quality_details"], "");
    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn integrated_target_scopes_do_not_require_the_retired_candidate_worktree() {
    for reconciliation in [false, true] {
        let fixture = goal_quality_fixture(
            if reconciliation {
                "quality-integrated-reconciliation"
            } else {
                "quality-integrated-target"
            },
            "printf provider-must-not-launch > provider-launched",
        );
        FileQualityService::new(&fixture.refine_dir)
            .save_settings(QualitySettingsPatch {
                tests: Some(Vec::new()),
                ..QualitySettingsPatch::default()
            })
            .unwrap();
        let work_items = FileWorkItemService::new(&fixture.refine_dir);
        let detail = work_items.show_goal_detail("GOAL1").unwrap();
        let candidate = detail["candidate_commit"].as_str().unwrap().to_string();
        let target_branch = git_output(&fixture.candidate_root, &["branch", "--show-current"])
            .trim()
            .to_string();
        work_items
            .set_goal_branch_name("GOAL1", "refine/retired/round-1")
            .unwrap();
        let mut evidence = json!({
            "workflow_quality_timing": "post_build",
            "workflow_integration": {
                "candidate_commit": candidate,
                "target_branch": target_branch,
                "target_commit": candidate,
                "remote": "origin",
                "pushed": false,
                "integrated_at": "2026-01-01T00:00:00Z",
                "merge": {"ok": true, "conflicts": [], "message": "integrated"}
            }
        });
        if reconciliation {
            evidence["workflow_reconciliation"] = json!({
                "state": "detected",
                "candidate_commit": candidate
            });
        }
        work_items
            .update_goal_round_evaluation_summary("GOAL1", 0, &evidence)
            .unwrap();
        assert!(
            FileGitWorktreeService::new(&fixture.candidate_root)
                .existing_worktree_for_branch("refine/retired/round-1")
                .unwrap()
                .is_none()
        );

        let operation = fixture
            .runner()
            .run_goal_checks("GOAL1", "smoke-ai", Default::default())
            .unwrap();
        assert!(operation.result.ok);
        assert_eq!(
            operation.operation.request["evaluation_scope"],
            if reconciliation {
                "isolated_candidate"
            } else {
                "integrated_target"
            }
        );
        assert!(!fixture.candidate_root.join("provider-launched").exists());
        fs::remove_dir_all(fixture.temp_root).unwrap();
    }
}
