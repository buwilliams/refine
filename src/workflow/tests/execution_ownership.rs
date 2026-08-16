use super::*;

#[test]
fn tracked_dirt_in_the_shared_checkout_no_longer_holds_the_goal() {
    let temp_root = unique_temp_dir("workspace-hold");
    let target_root = temp_root.join("repo");
    let runtime_root = temp_root.join("run/8080");
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
    git(&target_root, &["commit", "-q", "-m", "base"]).unwrap();
    let base = git_output(&target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();

    let refine_dir = test_refine_dir(&target_root);
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Held by user work", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
        .unwrap();
    work_items
        .update_goal_git_refs("GOAL1", "refine/GOAL1", "main", &base, Some(&base))
        .unwrap();

    // A human's uncommitted tracked edit in the shared checkout.
    fs::write(target_root.join("app.txt"), "uncommitted human edit\n").unwrap();

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    // The Goal may still fail on its own merits (this round has no Git
    // evidence to integrate); what matters is that the pass processed it.
    let _ = workflow.execute_work();

    // Integration porcelain runs in the detached integration worktree, so
    // shared-checkout dirt no longer parks the Goal behind a workspace hold,
    // and the human's edit stayed untouched.
    let holds = fs::read_to_string(runtime_root.join("scheduler-holds.jsonl")).unwrap_or_default();
    assert!(
        !holds
            .lines()
            .any(|line| line.contains("\"GOAL1\"") && line.contains("workspace_hold")),
        "{holds}"
    );
    assert_eq!(
        fs::read_to_string(target_root.join("app.txt")).unwrap(),
        "uncommitted human edit\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

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
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
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
        GoalStatus::Plan
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
        ("ZERO1", GoalStatus::Implement),
        ("ZERO2", GoalStatus::Governance),
        ("ZERO3", GoalStatus::Governance),
        ("ZERO4", GoalStatus::Quality),
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
        .set_goal_status_unchecked("VALID1", &GoalStatus::Implement)
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
fn cleanup_observes_candidate_handoff_through_governance_integration() {
    use crate::process::supervisor::operations::{FileOperationRegistry, OperationState};
    use crate::tools::host::git_sync::with_repository_git_lock;
    use crate::tools::product::worktree_cleanup::{
        FileWorktreeCleanupService, WorktreeCleanupOptions,
    };
    use crate::workflow::candidate_handoff::{
        record_candidate_handoff_commit, record_candidate_handoff_governance,
        register_candidate_handoff, settle_candidate_handoff,
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
    record_candidate_handoff_governance(&runtime_root, &handoff.id, base.trim()).unwrap();
    let during_governance = cleanup
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(during_governance.removed, 0);
    assert!(worktree.exists());

    settle_candidate_handoff(
        &runtime_root,
        &handoff.id,
        "candidate_integrated",
        json!({"candidate_commit": base.trim()}),
    )
    .unwrap();
    let after_integration = cleanup
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(after_integration.removed, 1);
    assert!(!worktree.exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn newer_round_handoff_supersedes_only_the_same_goal_candidate() {
    use crate::process::supervisor::operations::{
        FileOperationRegistry, OperationRegistry, OperationState,
    };
    use crate::workflow::candidate_handoff::{
        register_candidate_handoff, retain_candidate_handoff_after_failure,
    };

    let temp_root = unique_temp_dir("candidate-handoff-supersession");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("target");
    let other_target = temp_root.join("other-target");
    fs::create_dir_all(&target_root).unwrap();
    fs::create_dir_all(&other_target).unwrap();

    let first = register_candidate_handoff(
        &runtime_root,
        &target_root,
        "GOAL1",
        0,
        "default",
        "refine/GOAL1/round-1",
        "/tmp/goal1-round-1",
        "base-1",
    )
    .unwrap();
    retain_candidate_handoff_after_failure(
        &runtime_root,
        &first.id,
        "governance_failed",
        Some("quality-candidate"),
        &RefineError::Conflict("candidate requires another Round".to_string()),
    );
    let retained = FileOperationRegistry::new(&runtime_root)
        .status(&first.id)
        .unwrap();
    assert_eq!(retained.state, OperationState::Running);
    assert_eq!(
        retained.progress["stage"],
        "workflow_failed_candidate_retained"
    );
    assert_eq!(retained.progress["failure"]["code"], "governance_failed");
    assert_eq!(retained.progress["candidate_commit"], "quality-candidate");
    let unrelated = register_candidate_handoff(
        &runtime_root,
        &other_target,
        "GOAL1",
        0,
        "default",
        "refine/GOAL1/round-1",
        "/tmp/other-goal1-round-1",
        "base-1",
    )
    .unwrap();
    let successor = register_candidate_handoff(
        &runtime_root,
        &target_root,
        "GOAL1",
        1,
        "default",
        "refine/GOAL1/round-2",
        "/tmp/goal1-round-2",
        "base-2",
    )
    .unwrap();

    let registry = FileOperationRegistry::new(&runtime_root);
    let first = registry.status(&first.id).unwrap();
    assert_eq!(first.state, OperationState::Succeeded);
    assert_eq!(first.result["disposition"], "superseded_by_round");
    assert_eq!(first.result["evidence"]["successor_round_idx"], 1);
    assert_eq!(
        first.result["evidence"]["successor_operation_id"],
        successor.id
    );
    assert_eq!(
        registry.status(&successor.id).unwrap().state,
        OperationState::Running
    );
    assert_eq!(
        registry.status(&unrelated.id).unwrap().state,
        OperationState::Running
    );

    fs::remove_dir_all(temp_root).unwrap();
}
