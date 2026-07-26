use super::*;

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
fn ready_merge_still_rejects_a_candidate_missing_from_the_target_branch() {
    let (temp_root, target_root, worktree_path, work_items, _candidate_commit) =
        ready_merge_goal_with_advanced_target("ready-merge-genuinely-stale", false);
    let runtime_root = temp_root.join("run/8080");

    let error = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap_err();

    // Work that is genuinely absent from the target must still be refused:
    // merging it would risk dropping what moved the tip in between.
    assert!(
        error.to_string().contains("is stale"),
        "expected the staleness guard to hold, got {error}"
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Failed
    );
    assert!(!target_root.join("feature.txt").exists());
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["rounds"][0]["failure_category"], "merge");
    assert!(
        detail["rounds"][0]["failure_message"]
            .as_str()
            .unwrap_or_default()
            .contains("is stale"),
        "the Goal must record why it failed, got {:?}",
        detail["rounds"][0]["failure_message"]
    );

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
        decision_version: 1,
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
