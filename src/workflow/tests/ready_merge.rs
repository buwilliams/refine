use super::*;

fn failed_goal_with_integrated_candidate(
    label: &str,
) -> (
    PathBuf,
    PathBuf,
    PathBuf,
    FileWorkItemService,
    String,
    String,
) {
    let (temp_root, target_root, worktree_path, work_items, candidate_commit) =
        ready_merge_goal_with_advanced_target(label, true);
    let merge_commit = git_stdout(
        &target_root,
        &["rev-list", "--first-parent", "--merges", "-n", "1", "HEAD"],
    )
    .unwrap()
    .trim()
    .to_string();
    work_items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "workflow_integration": {
                    "candidate_commit": candidate_commit,
                    "target_branch": "main",
                    "target_commit": merge_commit,
                    "remote": "origin",
                    "pushed": false,
                    "integrated_at": now_timestamp(),
                    "merge": {
                        "ok": true,
                        "conflicts": [],
                        "message": "Integrated candidate before post-merge failure"
                    }
                }
            }),
        )
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    (
        temp_root,
        target_root,
        worktree_path,
        work_items,
        candidate_commit,
        merge_commit,
    )
}

fn reconciliation_runtime_root(temp_root: &Path) -> PathBuf {
    temp_root.with_file_name(format!(
        "{}-runtime",
        temp_root.file_name().unwrap().to_string_lossy()
    ))
}

#[test]
fn requeued_already_merged_goal_runs_quality_on_target_and_finishes_done() {
    let (temp_root, target_root, worktree_path, work_items, candidate_commit, _merge_commit) =
        failed_goal_with_integrated_candidate("already-merged-reconcile-pass");
    let runtime_root = reconciliation_runtime_root(&temp_root);
    let original_target = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();

    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].commit, candidate_commit);
    assert_eq!(result.steps[0].final_status, "done");
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Done
    );
    assert_eq!(
        git_stdout(&target_root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        original_target
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["state"],
        "completed"
    );
    assert_eq!(
        detail["rounds"][0]["quality_details"]["evaluation_scope"],
        "integrated_target_reconciliation"
    );
    let messages = detail["rounds"][0]["logs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|log| log["message"].as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Detected already-merged candidate"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("reconciled as done"))
    );

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn terminal_reconciliation_is_rechecked_when_the_candidate_is_present() {
    for recorded_state in ["reverted", "completed"] {
        let label = format!("terminal-reconciliation-present-{recorded_state}");
        let (temp_root, target_root, worktree_path, work_items, candidate_commit, _) =
            failed_goal_with_integrated_candidate(&label);
        let runtime_root = reconciliation_runtime_root(&temp_root);
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({
                    "workflow_reconciliation": {
                        "state": recorded_state
                    }
                }),
            )
            .unwrap();

        let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
            .evaluate_workflow()
            .unwrap();

        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].final_status, "done");
        assert_eq!(result.steps[0].commit, candidate_commit);
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Done
        );
        let detail = work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(
            detail["rounds"][0]["workflow_reconciliation"]["state"],
            "completed"
        );
        assert!(
            detail["rounds"][0]["logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|log| {
                    log["message"].as_str().is_some_and(|message| {
                        message.contains(
                            "Recorded reconciliation state was stale; the candidate is currently present",
                        )
                    }) && log["details"]["recorded_reconciliation_state"] == recorded_state
                })
        );

        fs::remove_dir_all(&worktree_path).ok();
        fs::remove_dir_all(temp_root).unwrap();
        fs::remove_dir_all(runtime_root).unwrap();
    }
}

#[test]
fn absent_terminal_reconciliation_queues_a_fresh_round_without_a_retryable_conflict() {
    for recorded_state in ["reverted", "completed"] {
        let label = format!("terminal-reconciliation-absent-{recorded_state}");
        let (temp_root, target_root, worktree_path, work_items, candidate_commit) =
            ready_merge_goal_with_advanced_target(&label, false);
        let runtime_root = reconciliation_runtime_root(&temp_root);
        let target_commit = git_stdout(&target_root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({
                    "workflow_integration": {
                        "candidate_commit": candidate_commit,
                        "target_branch": "main",
                        "target_commit": target_commit,
                        "remote": "origin",
                        "pushed": false,
                        "integrated_at": now_timestamp(),
                        "merge": {
                            "ok": true,
                            "conflicts": [],
                            "message": "Recorded prior integration"
                        }
                    },
                    "workflow_reconciliation": {
                        "state": recorded_state
                    }
                }),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();

        let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
            .evaluate_workflow()
            .unwrap();

        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].final_status, "todo");
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Todo
        );
        assert!(!target_root.join("feature.txt").exists());
        assert!(worktree_path.join("feature.txt").exists());
        let detail = work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
        assert_eq!(
            detail["rounds"][0]["failure_category"],
            "reconciliation_candidate_absent"
        );
        assert_eq!(
            detail["rounds"][0]["workflow_recovery"]["state"],
            "superseded"
        );
        assert_eq!(
            detail["rounds"][0]["workflow_recovery"]["recorded_reconciliation_state"],
            recorded_state
        );
        assert_eq!(detail["rounds"][1]["workflow_recovery"]["state"], "queued");
        assert_eq!(
            detail["rounds"][1]["workflow_recovery"]["candidate_commit"],
            candidate_commit
        );
        assert!(
            detail["rounds"][0]["logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|log| log["message"]
                    .as_str()
                    .is_some_and(|message| { message.contains("queued a fresh recovery round") }))
        );
        assert!(
            result
                .claims
                .iter()
                .all(|claim| { claim.failure_stage.as_deref() != Some("preparation") })
        );
        assert!(result.claims.iter().any(|claim| {
            claim.goal_id == "GOAL1" && claim.state == WorkflowClaimState::Completed
        }));

        fs::remove_dir_all(&worktree_path).ok();
        fs::remove_dir_all(temp_root).unwrap();
        fs::remove_dir_all(runtime_root).unwrap();
    }
}

#[test]
fn candidate_mismatch_preparation_failure_is_quarantined_without_starving_the_next_goal() {
    let (temp_root, target_root, worktree_path, work_items, _, _) =
        failed_goal_with_integrated_candidate("candidate-mismatch-preparation-quarantine");
    let runtime_root = reconciliation_runtime_root(&temp_root);
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    let branch = detail["branch_name"].as_str().unwrap();
    let target_branch = detail["target_branch"].as_str().unwrap();
    let base_commit = detail["base_commit"].as_str().unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL1",
            branch,
            target_branch,
            base_commit,
            Some("1111111111111111111111111111111111111111"),
        )
        .unwrap();
    work_items
        .create_goal_summary("Runnable sibling", Some("GOAL2"))
        .unwrap();
    work_items
        .update_goal_metadata_summary("GOAL1", None, Some("high"), None, None)
        .unwrap();
    work_items
        .update_goal_metadata_summary("GOAL2", None, Some("low"), None, None)
        .unwrap();
    work_items
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(test_refine_dir(&target_root))
        .update(&json!({
            "parallel_run_cap": 1,
            "parallel_per_node_cap": 1,
            "parallel_per_provider_cap": 1,
            "parallel_per_target_app_cap": 1
        }))
        .unwrap();
    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);

    let error = automation.evaluate_workflow().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("candidate changed from integrated commit")
    );

    let state = automation.load_state().unwrap();
    let failed = state
        .claims
        .iter()
        .find(|claim| claim.goal_id == "GOAL1")
        .unwrap();
    assert_eq!(failed.state, WorkflowClaimState::Failed);
    assert_eq!(failed.failure_stage.as_deref(), Some("preparation"));
    assert!(failed.goal_revision.is_some());
    assert!(
        failed.failure_message.as_deref().is_some_and(|message| {
            message.contains("candidate changed from integrated commit")
        })
    );
    assert_eq!(
        automation.preparation_failures_needing_attention().unwrap(),
        vec![failed.clone()]
    );

    // The deterministic failure is quarantined at this exact Goal revision.
    // The next pass can admit its sibling instead of reclaiming GOAL1 forever.
    assert_eq!(automation.promote().unwrap(), 1);
    let state = automation.load_state().unwrap();
    assert_eq!(
        state
            .claims
            .iter()
            .filter(|claim| claim.goal_id == "GOAL1")
            .count(),
        1
    );
    assert!(
        state.claims.iter().any(|claim| {
            claim.goal_id == "GOAL2" && claim.state == WorkflowClaimState::Claimed
        })
    );

    // A durable Goal mutation changes its revision and makes an automated retry
    // eligible again once the sibling no longer occupies the only slot.
    let sibling_claim = state
        .claims
        .iter()
        .find(|claim| claim.goal_id == "GOAL2")
        .unwrap()
        .claim_id
        .clone();
    automation
        .mark_claim_state(&sibling_claim, None, WorkflowClaimState::Cancelled)
        .unwrap();
    work_items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({"operator_recovery_evidence": "fresh"}),
        )
        .unwrap();
    assert_eq!(automation.promote().unwrap(), 1);
    let state = automation.load_state().unwrap();
    assert_eq!(
        state
            .claims
            .iter()
            .filter(|claim| claim.goal_id == "GOAL1")
            .count(),
        2
    );
    assert!(
        automation
            .preparation_failures_needing_attention()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn parallel_already_merged_reconciliations_serialize_shared_target_quality() {
    let (temp_root, target_root, first_worktree, work_items, _, _) =
        failed_goal_with_integrated_candidate("already-merged-reconcile-parallel");
    let runtime_root = reconciliation_runtime_root(&temp_root);
    fs::create_dir_all(&runtime_root).unwrap();

    let second_base = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    let second_branch = "refine/GOAL2/round-1";
    let second_worktree = target_root
        .join(".git/refine-worktrees")
        .join(second_branch.replace('/', "-"));
    fs::create_dir_all(second_worktree.parent().unwrap()).unwrap();
    git(
        &target_root,
        &[
            "worktree",
            "add",
            "-b",
            second_branch,
            second_worktree.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::write(
        second_worktree.join("feature-two.txt"),
        "second candidate\n",
    )
    .unwrap();
    git(&second_worktree, &["add", "feature-two.txt"]).unwrap();
    git(
        &second_worktree,
        &["commit", "-q", "-m", "second candidate"],
    )
    .unwrap();
    let second_candidate = git_stdout(&second_worktree, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    git(
        &target_root,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge second candidate",
            second_branch,
        ],
    )
    .unwrap();
    let second_merge = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    work_items
        .create_goal_summary("Second integrated Goal", Some("GOAL2"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL2", "Reporter", "Prompt")
        .unwrap();
    work_items
        .update_latest_goal_round_implementation_report("GOAL2", "Implementation completed")
        .unwrap();
    work_items
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL2", GoalStatus::InProgress)
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL2",
            second_branch,
            "main",
            &second_base,
            Some(&second_candidate),
        )
        .unwrap();
    work_items
        .update_goal_round_evaluation_summary(
            "GOAL2",
            0,
            &json!({
                "workflow_quality_timing": "pre_merge",
                "workflow_git_remote": "origin",
                "workflow_integration": {
                    "candidate_commit": second_candidate,
                    "target_branch": "main",
                    "target_commit": second_merge,
                    "remote": "origin",
                    "pushed": false,
                    "integrated_at": now_timestamp(),
                    "merge": {
                        "ok": true,
                        "conflicts": [],
                        "message": "Integrated second candidate before post-merge failure"
                    }
                }
            }),
        )
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL2", GoalStatus::ReadyMerge)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL2", GoalStatus::Failed)
        .unwrap();
    work_items
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();

    let collision = runtime_root.join("quality-collision");
    let active = runtime_root.join("quality-active");
    let quality_command = runtime_root.join("quality-command");
    fs::write(
        &quality_command,
        format!(
            "#!/bin/sh\nif mkdir '{}' 2>/dev/null; then\n  trap 'rmdir \"{}\" 2>/dev/null' EXIT\n  attempt=0\n  while [ ! -f '{}' ] && [ \"$attempt\" -lt 100 ]; do\n    sleep 0.01\n    attempt=$((attempt + 1))\n  done\n  exit 0\nfi\ntouch '{}'\nexit 1\n",
            active.display(),
            active.display(),
            collision.display(),
            collision.display()
        ),
    )
    .unwrap();
    let smoke_ai = runtime_root.join("quality-smoke-ai");
    fs::write(
        &smoke_ai,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"summary\":\"Quality planned.\",\"results\":[{{\"test\":\"Shared target build is exclusive\",\"status\":\"passed\",\"evidence\":\"planned supervised check\",\"command\":\"{}\"}}]}}'\n",
            quality_command.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        for script in [&quality_command, &smoke_ai] {
            let mut permissions = fs::metadata(script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(script, permissions).unwrap();
        }
    }
    FileQualityService::new(test_refine_dir(&target_root))
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Shared target build is exclusive".to_string()]),
            ..Default::default()
        })
        .unwrap();
    FileSettingsService::new(test_refine_dir(&target_root))
        .update(&json!({
            "agent_cli": "smoke-ai",
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "parallel_per_provider_cap": 2,
            "parallel_per_target_app_cap": 2
        }))
        .unwrap();
    let original_target = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai) };

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();

    assert_eq!(result.steps.len(), 2);
    assert!(result.steps.iter().all(|step| step.final_status == "done"));
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Done
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Done
    );
    for goal_id in ["GOAL1", "GOAL2"] {
        let detail = work_items.show_goal_detail(goal_id).unwrap();
        assert_eq!(
            detail["rounds"][0]["workflow_reconciliation"]["state"],
            "completed"
        );
        assert!(
            detail["rounds"][0]["logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|log| log["message"].as_str()
                    == Some("Acquired exclusive integrated-target workflow lane"))
        );
    }
    assert!(!collision.exists());
    assert!(target_root.join("feature.txt").exists());
    assert!(target_root.join("feature-two.txt").exists());
    assert_eq!(
        git_stdout(&target_root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        original_target
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(&first_worktree).ok();
    fs::remove_dir_all(&second_worktree).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn direct_quality_retry_also_reconciles_an_already_merged_candidate() {
    let (temp_root, target_root, worktree_path, work_items, candidate_commit, _merge_commit) =
        failed_goal_with_integrated_candidate("already-merged-direct-quality-retry");
    let runtime_root = reconciliation_runtime_root(&temp_root);
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    work_items.retry_goal_quality_summary("GOAL1").unwrap();

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();

    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].commit, candidate_commit);
    assert_eq!(result.steps[0].final_status, "done");
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Done
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["state"],
        "completed"
    );
    assert!(
        detail["rounds"][0]["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|log| log["message"]
                .as_str()
                .is_some_and(|message| message.contains("while resuming workflow")))
    );

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn requeued_already_merged_goal_reverts_exact_merge_before_failing_quality() {
    let (temp_root, target_root, worktree_path, work_items, candidate_commit, merge_commit) =
        failed_goal_with_integrated_candidate("already-merged-reconcile-fail");
    let runtime_root = reconciliation_runtime_root(&temp_root);
    fs::create_dir_all(&runtime_root).unwrap();
    let smoke_ai = runtime_root.join("quality-smoke-ai");
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":false,\"summary\":\"Quality failed.\",\"results\":[{\"test\":\"Feature remains valid\",\"status\":\"failed\",\"evidence\":\"observed failure\",\"command\":\"false\"}]}'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    FileQualityService::new(test_refine_dir(&target_root))
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Feature remains valid".to_string()]),
            ..Default::default()
        })
        .unwrap();
    FileSettingsService::new(test_refine_dir(&target_root))
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai) };

    let error = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("already-merged candidate was reverted")
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Failed
    );
    assert!(!target_root.join("feature.txt").exists());
    assert!(target_root.join("unrelated.txt").exists());
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["state"],
        "reverted"
    );
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["candidate_commit"],
        candidate_commit
    );
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["merge_commit"],
        merge_commit
    );
    let revert_commit = detail["rounds"][0]["workflow_reconciliation"]["revert_commit"]
        .as_str()
        .unwrap();
    assert_eq!(
        git_stdout(&target_root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        revert_commit
    );

    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn dirty_target_blocks_already_merged_reconciliation_without_false_failure_or_revert() {
    let (temp_root, target_root, worktree_path, work_items, _candidate_commit, _merge_commit) =
        failed_goal_with_integrated_candidate("already-merged-reconcile-dirty");
    let runtime_root = reconciliation_runtime_root(&temp_root);
    fs::write(target_root.join("operator.txt"), "preserve me\n").unwrap();

    let error = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unattributed dirty index or worktree")
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Qa
    );
    assert!(target_root.join("feature.txt").exists());
    assert_eq!(
        fs::read_to_string(target_root.join("operator.txt")).unwrap(),
        "preserve me\n"
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["state"],
        "detected"
    );
    assert_eq!(detail["rounds"][0]["quality_state"], "unclassified");
    assert!(
        detail["rounds"][0]["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|log| log["message"]
                .as_str()
                .is_some_and(|message| message.contains("Goal status was preserved")))
    );

    fs::remove_file(target_root.join("operator.txt")).unwrap();
    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn file_automation_resumes_supported_ready_merge_retry_without_rerunning_implementation() {
    let temp_root = unique_temp_dir("automation-ready-merge-retry");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "base\n").unwrap();
    git(&temp_root, &["init", "-q", "-b", "main"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    let base_commit = git_stdout(&target_root, &["rev-parse", "HEAD"]).unwrap();
    let branch = "refine/GOAL1/round-1";
    let worktree_path = temp_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
    git(
        &target_root,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            worktree_path.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::write(worktree_path.join("feature.txt"), "retry candidate\n").unwrap();
    git(&worktree_path, &["add", "feature.txt"]).unwrap();
    git(&worktree_path, &["commit", "-q", "-m", "retry candidate"]).unwrap();
    let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]).unwrap();

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Retry Ready Merge", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .update_latest_goal_round_implementation_report("GOAL1", "Implementation already completed")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL1",
            branch,
            "main",
            base_commit.trim(),
            Some(candidate_commit.trim()),
        )
        .unwrap();
    work_items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "workflow_quality_timing": "pre_merge",
                "workflow_git_remote": "origin"
            }),
        )
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    work_items.retry_goal_merge_summary("GOAL1").unwrap();

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].commit, candidate_commit.trim());
    assert_eq!(
        result.steps[0].provider_output,
        "Implementation already completed"
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    assert_eq!(
        fs::read_to_string(target_root.join("feature.txt")).unwrap(),
        "retry candidate\n"
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_integration"]["candidate_commit"],
        candidate_commit.trim()
    );
    assert!(
        detail["rounds"][0]["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|log| log["message"] == "Resumed workflow from ready-merge")
    );

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn parallel_post_build_goals_serialize_the_entire_integrated_target_lane() {
    let temp_root = unique_temp_dir("parallel-post-build-lane");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&target_root).unwrap();
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(target_root.join("app.py"), "base\n").unwrap();
    git(&target_root, &["init", "-q", "-b", "main"]).unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.py"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "base"]).unwrap();
    let base_commit = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    let work_items = FileWorkItemService::new(&refine_dir);
    let mut worktrees = Vec::new();
    for (goal_id, feature) in [("GOAL1", "one"), ("GOAL2", "two")] {
        let branch = format!("refine/{goal_id}/round-1");
        let worktree = target_root
            .join(".git/refine-worktrees")
            .join(branch.replace('/', "-"));
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            &target_root,
            &["worktree", "add", "-b", &branch, worktree.to_str().unwrap()],
        )
        .unwrap();
        let feature_path = format!("feature-{feature}.txt");
        fs::write(worktree.join(&feature_path), format!("{feature}\n")).unwrap();
        git(&worktree, &["add", &feature_path]).unwrap();
        git(&worktree, &["commit", "-q", "-m", feature]).unwrap();
        let candidate = git_stdout(&worktree, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        work_items
            .create_goal_summary(feature, Some(goal_id))
            .unwrap();
        work_items
            .append_goal_round_summary(goal_id, "Reporter", "Prompt")
            .unwrap();
        work_items
            .update_latest_goal_round_implementation_report(goal_id, "Implementation completed")
            .unwrap();
        work_items
            .transition_goal_status(goal_id, GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status(goal_id, GoalStatus::InProgress)
            .unwrap();
        work_items
            .update_goal_git_refs(goal_id, &branch, "main", &base_commit, Some(&candidate))
            .unwrap();
        work_items
            .update_goal_round_evaluation_summary(
                goal_id,
                0,
                &json!({
                    "workflow_quality_timing": "post_build",
                    "workflow_git_remote": "origin"
                }),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status(goal_id, GoalStatus::ReadyMerge)
            .unwrap();
        work_items
            .advance_automated_goal_status(goal_id, GoalStatus::Failed)
            .unwrap();
        work_items.retry_goal_merge_summary(goal_id).unwrap();
        worktrees.push(worktree);
    }

    let collision = runtime_root.join("build-collision");
    let active = runtime_root.join("build-active");
    let build = runtime_root.join("exclusive-build");
    fs::write(
        &build,
        format!(
            "#!/bin/sh\nif mkdir '{}' 2>/dev/null; then\n  trap 'rmdir \"{}\" 2>/dev/null' EXIT\n  attempt=0\n  while [ ! -f '{}' ] && [ \"$attempt\" -lt 100 ]; do\n    sleep 0.01\n    attempt=$((attempt + 1))\n  done\n  exit 0\nfi\ntouch '{}'\nexit 1\n",
            active.display(),
            active.display(),
            collision.display(),
            collision.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&build).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&build, permissions).unwrap();
    }
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_build_command": build.display().to_string(),
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "parallel_per_provider_cap": 2,
            "parallel_per_target_app_cap": 2
        }))
        .unwrap();

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();

    assert_eq!(result.steps.len(), 2);
    assert!(!collision.exists());
    for goal_id in ["GOAL1", "GOAL2"] {
        assert_eq!(
            work_items.show_goal_summary(goal_id).unwrap().goal.status,
            GoalStatus::Review
        );
        let detail = work_items.show_goal_detail(goal_id).unwrap();
        assert_eq!(
            detail["rounds"][0]["quality_details"]["evaluation_scope"],
            "integrated_target"
        );
        assert!(
            detail["rounds"][0]["logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|log| log["message"] == "Acquired exclusive integrated-target workflow lane")
        );
    }
    assert!(target_root.join("feature-one.txt").exists());
    assert!(target_root.join("feature-two.txt").exists());

    for worktree in worktrees {
        fs::remove_dir_all(worktree).ok();
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn build_mutation_is_failed_and_attributed_before_review() {
    let (temp_root, target_root, worktree_path, work_items, _) =
        ready_merge_goal_with_advanced_target("build-mutation-attribution", true);
    let runtime_root = reconciliation_runtime_root(&temp_root);
    fs::create_dir_all(&runtime_root).unwrap();
    let build = runtime_root.join("mutating-build");
    fs::write(
        &build,
        "#!/bin/sh\nprintf 'build mutation\\n' >> app.py\ngit add app.py\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&build).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&build, permissions).unwrap();
    }
    FileSettingsService::new(test_refine_dir(&target_root))
        .update(&json!({"target_app_build_command": build.display().to_string()}))
        .unwrap();

    let error = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("integrated-target after build found a dirty")
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Failed
    );
    assert_eq!(
        fs::read_to_string(target_root.join("app.py")).unwrap(),
        "base\nbuild mutation\n"
    );
    let marker = crate::tools::host::git_worktrees::FileGitWorktreeService::new(&target_root)
        .git_path("refine-integrated-target-transaction.json")
        .unwrap();
    assert!(marker.exists());

    fs::remove_dir_all(worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
    fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn ready_merge_accepts_a_candidate_already_present_in_the_target_branch() {
    let (temp_root, target_root, worktree_path, work_items, candidate_commit) =
        ready_merge_goal_with_advanced_target("ready-merge-already-integrated", true);
    let runtime_root = temp_root.join("run/8080");

    WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .expect("a candidate already in the target branch has nothing left to merge");

    // The work is in the branch whatever the recorded base says, so the Goal
    // must not be failed as stale over it.
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    assert_eq!(
        fs::read_to_string(target_root.join("feature.txt")).unwrap(),
        "candidate work\n"
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_integration"]["candidate_commit"],
        candidate_commit
    );

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn ready_merge_queues_a_fresh_round_when_the_candidate_is_stale() {
    let (temp_root, target_root, worktree_path, work_items, candidate_commit) =
        ready_merge_goal_with_advanced_target("ready-merge-genuinely-stale", false);
    let runtime_root = temp_root.join("run/8080");
    let current_target = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();

    // Work that is genuinely absent from the target must still be refused, but
    // branch movement is recoverable: preserve the candidate and queue a fresh
    // auditable round from the current target instead of terminally failing it.
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].final_status, "todo");
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert!(!target_root.join("feature.txt").exists());
    assert!(worktree_path.join("feature.txt").exists());
    assert_eq!(
        git_stdout(&worktree_path, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        candidate_commit
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(detail["rounds"][0]["failure_category"], "stale_candidate");
    assert!(
        detail["rounds"][0]["failure_message"]
            .as_str()
            .unwrap_or_default()
            .contains("is stale"),
        "the Goal must record why it failed, got {:?}",
        detail["rounds"][0]["failure_message"]
    );
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["state"],
        "superseded"
    );
    assert_eq!(detail["rounds"][1]["workflow_recovery"]["state"], "queued");
    assert_eq!(
        detail["rounds"][1]["workflow_recovery"]["candidate_commit"],
        candidate_commit
    );
    assert_eq!(
        detail["rounds"][1]["workflow_recovery"]["target_commit"],
        current_target
    );
    assert!(
        detail["rounds"][1]["prompt"]
            .as_str()
            .unwrap_or_default()
            .contains(&candidate_commit)
    );
    assert!(
        detail["rounds"][0]["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|log| log["message"]
                .as_str()
                .is_some_and(|message| message.contains("queued a fresh recovery round")))
    );
    let operation_errors = fs::read_dir(runtime_root.join("operations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .map(|entry| {
            serde_json::from_str::<Value>(&fs::read_to_string(entry.path()).unwrap()).unwrap()
        })
        .filter_map(|operation| operation.get("error").cloned())
        .collect::<Vec<_>>();
    assert!(operation_errors.iter().any(|error| {
        error["code"] == "ready_merge_candidate_stale"
            && error["message"]
                .as_str()
                .is_some_and(|message| message.contains("is stale"))
    }));

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn ready_merge_fence_rejects_cancellation_replacement_and_unequal_claims() {
    let temp_root = unique_temp_dir("ready-merge-execution-fence");
    let runtime_root = temp_root.join("run/8080");
    let automation = WorkflowEngine::new(&runtime_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    let execution_id = automation.start_claim(&claim_id).unwrap();
    let fence = automation
        .commit_ready_merge_fence(&claim_id, &execution_id, "GOAL1", "default", 0, 7)
        .unwrap();
    automation.verify_ready_merge_fence(&fence).unwrap();

    automation.cancel(&execution_id).unwrap();
    assert!(
        automation
            .verify_ready_merge_fence(&fence)
            .unwrap_err()
            .to_string()
            .contains("no longer owns")
    );
    let replacement_execution = automation.retry(&execution_id).unwrap();
    let replacement = automation
        .commit_ready_merge_fence(&claim_id, &replacement_execution, "GOAL1", "default", 0, 8)
        .unwrap();
    assert_ne!(replacement.execution_id, fence.execution_id);

    let mut state = automation.load_state().unwrap();
    state.claims.push(WorkflowClaim {
        claim_id: "unequal-claim".to_string(),
        goal_id: "GOAL1".to_string(),
        node_id: "default".to_string(),
        provider: "smoke-ai".to_string(),
        target_app_id: "default".to_string(),
        execution_id: Some("unequal-execution".to_string()),
        round_idx: Some(0),
        goal_revision: Some(9),
        failure_stage: None,
        failure_message: None,
        decision_version: 1,
        occurrences: 1,
        state: WorkflowClaimState::Running,
        created_at: now_timestamp(),
        updated_at: now_timestamp(),
    });
    automation.save_state(&mut state).unwrap();
    let error = automation
        .verify_ready_merge_fence(&replacement)
        .unwrap_err();
    assert!(error.to_string().contains("unequal concurrent claims"));
    assert!(error.to_string().contains("revision 8"));
    assert!(error.to_string().contains("revision 9"));

    fs::remove_dir_all(temp_root).unwrap();
}
