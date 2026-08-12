use super::*;

#[test]
fn restart_recovery_preserves_goal_state_and_removes_retired_execution_files() {
    let temp_root = unique_temp_dir("execution-ownership-recovery");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Restartable", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Continue idempotently")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();

    fs::create_dir_all(runtime_root.join("operations/.workflow-cancellations")).unwrap();
    for name in [
        "workflow-automation-state.json",
        ".workflow-automation-state.lock",
        "agent-capacity-state.json",
        ".agent-capacity.lock",
        ".workflow-process-registration.lock",
    ] {
        fs::create_dir_all(&runtime_root).unwrap();
        fs::write(runtime_root.join(name), b"legacy").unwrap();
    }
    fs::write(
        runtime_root.join("operations/.workflow-cancellations/GOAL1.json"),
        b"{}",
    )
    .unwrap();

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        workflow.recover_interrupted_goals("test restart").unwrap(),
        1
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::InProgress
    );
    for name in [
        "workflow-automation-state.json",
        ".workflow-automation-state.lock",
        "agent-capacity-state.json",
        ".agent-capacity.lock",
        ".workflow-process-registration.lock",
        "operations/.workflow-cancellations",
    ] {
        assert!(!runtime_root.join(name).exists(), "{name} was not removed");
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn backlog_promotion_changes_only_goal_state_and_does_not_create_worktrees() {
    let temp_root = unique_temp_dir("state-only-promotion");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for index in 0..8 {
        let id = format!("GOAL{index}");
        work_items.create_goal_summary(&id, Some(&id)).unwrap();
        work_items
            .append_goal_round_summary(&id, "Reporter", "Authored request")
            .unwrap();
    }

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(workflow.promote().unwrap(), 8);
    for index in 0..8 {
        let id = format!("GOAL{index}");
        assert_eq!(
            work_items.show_goal_summary(&id).unwrap().goal.status,
            GoalStatus::Todo
        );
    }
    assert!(!target_root.join(".git/refine-worktrees").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn zero_round_backlog_is_ineligible_while_valid_sibling_promotes() {
    let temp_root = unique_temp_dir("zero-round-promotion");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Invalid", Some("ZERO1"))
        .unwrap();
    work_items
        .create_goal_summary("Valid", Some("VALID1"))
        .unwrap();
    work_items
        .append_goal_round_summary("VALID1", "Reporter", "Authoritative request")
        .unwrap();

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(workflow.promote().unwrap(), 1);
    assert_eq!(
        work_items.show_goal_summary("ZERO1").unwrap().goal.status,
        GoalStatus::Backlog
    );
    assert_eq!(
        work_items.show_goal_summary("VALID1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert!(!target_root.join(".git/refine-worktrees").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn restart_recovery_preserves_active_zero_round_goal_and_recovers_valid_sibling() {
    let temp_root = unique_temp_dir("zero-round-recovery");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    let invalid = [
        ("ZERO1", GoalStatus::InProgress),
        ("ZERO2", GoalStatus::ReadyMerge),
        ("ZERO3", GoalStatus::Build),
        ("ZERO4", GoalStatus::Qa),
    ];
    for (id, status) in &invalid {
        work_items.create_goal_summary("Invalid", Some(id)).unwrap();
        work_items.set_goal_status_unchecked(id, status).unwrap();
    }
    work_items
        .create_goal_summary("Valid", Some("VALID1"))
        .unwrap();
    work_items
        .append_goal_round_summary("VALID1", "Reporter", "Continue")
        .unwrap();
    work_items
        .set_goal_status_unchecked("VALID1", &GoalStatus::InProgress)
        .unwrap();
    let zero_records = invalid
        .iter()
        .map(|(id, _)| {
            let path = refine_dir.join(work_items.show_goal_summary(id).unwrap().goal.json_path);
            (path.clone(), fs::read(path).unwrap())
        })
        .collect::<Vec<_>>();

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(workflow.recover_interrupted_goals("restart").unwrap(), 1);
    for (path, before) in zero_records {
        assert_eq!(fs::read(path).unwrap(), before);
    }
    for (id, status) in invalid {
        assert_eq!(
            work_items.show_goal_summary(id).unwrap().goal.status,
            status
        );
    }
    assert!(!target_root.join(".git/refine-worktrees").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn cleanup_observes_candidate_handoff_until_quality_owns_the_worktree() {
    use crate::process::supervisor::operations::{FileOperationRegistry, OperationState};
    use crate::tools::host::git_sync::with_repository_git_lock;
    use crate::tools::product::worktree_cleanup::{
        FileWorktreeCleanupService, WorktreeCleanupOptions,
    };
    use crate::workflow::candidate_handoff::{
        record_candidate_handoff_commit, register_candidate_handoff, settle_candidate_handoff,
    };

    let temp_root = unique_temp_dir("candidate-handoff-cleanup");
    let target_root = temp_root.join("target");
    fs::create_dir_all(&target_root).unwrap();
    git(&target_root, &["init", "-b", "main"]).unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    git(&target_root, &["add", "app.txt"]).unwrap();
    git(&target_root, &["commit", "-m", "base"]).unwrap();
    let base = git_output(&target_root, &["rev-parse", "HEAD"]);
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let branch = "refine/GOAL1/round-1";
    let worktree = target_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &target_root,
        &["worktree", "add", "-b", branch, worktree.to_str().unwrap()],
    )
    .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Candidate handoff", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement")
        .unwrap();
    work_items.set_goal_branch_name("GOAL1", branch).unwrap();

    let handoff = with_repository_git_lock(&target_root, || {
        register_candidate_handoff(
            &runtime_root,
            &target_root,
            "GOAL1",
            0,
            "default",
            branch,
            worktree.to_str().unwrap(),
            base.trim(),
        )
    })
    .unwrap();
    record_candidate_handoff_commit(&runtime_root, &handoff.id, base.trim()).unwrap();
    let cleanup = FileWorktreeCleanupService::new(&target_root, &runtime_root);
    let during_handoff = cleanup
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(during_handoff.removed, 0);
    assert!(worktree.exists());

    let quality = with_repository_git_lock(&target_root, || {
        let registry = FileOperationRegistry::new(&runtime_root);
        let quality = registry.register_exclusive_with_request(
            "quality:GOAL1:candidate",
            json!({
                "kind": "quality",
                "goal_id": "GOAL1",
                "round_idx": 0,
                "worktree_path": worktree,
                "evaluation_scope": "isolated_candidate"
            }),
        )?;
        settle_candidate_handoff(
            &runtime_root,
            &handoff.id,
            "transferred_to_quality",
            json!({"quality_operation_id": quality.id}),
        )?;
        Ok(quality)
    })
    .unwrap();
    let during_quality = cleanup
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(during_quality.removed, 0);
    assert!(worktree.exists());

    FileOperationRegistry::new(&runtime_root)
        .finish_with_result(&quality.id, OperationState::Succeeded, json!({"ok": true}))
        .unwrap();
    let after_transfer = cleanup
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(after_transfer.removed, 1);
    assert!(!worktree.exists());
    fs::remove_dir_all(temp_root).unwrap();
}
