use super::*;

use crate::application::work_items::WorkflowAttemptAuthority;
use crate::application::workflow::engine::context::WorkflowContext;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};

struct RefreshFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    worktree: PathBuf,
    work_items: FileWorkItemService,
    base: String,
    candidate: String,
    target: String,
    authority: WorkflowAttemptAuthority,
}

impl RefreshFixture {
    fn new(conflict: bool) -> Self {
        Self::new_with_retry(conflict, None)
    }

    fn new_with_retry(conflict: bool, automatic_retry_attempt: Option<u32>) -> Self {
        let temp_root = unique_temp_dir("workflow-candidate-refresh");
        let target_root = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        let worktree = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
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
        let base = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git(
            &target_root,
            &[
                "worktree",
                "add",
                "-b",
                "refine/GOAL1/round-1",
                worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        if conflict {
            fs::write(worktree.join("app.txt"), "candidate\n").unwrap();
            git(&worktree, &["add", "app.txt"]).unwrap();
        } else {
            fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
            git(&worktree, &["add", "candidate.txt"]).unwrap();
        }
        git(&worktree, &["commit", "-m", "candidate"]).unwrap();
        let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        if conflict {
            fs::write(target_root.join("app.txt"), "target\n").unwrap();
            git(&target_root, &["add", "app.txt"]).unwrap();
        } else {
            fs::write(target_root.join("sibling.txt"), "sibling\n").unwrap();
            git(&target_root, &["add", "sibling.txt"]).unwrap();
        }
        git(&target_root, &["commit", "-m", "advance target"]).unwrap();
        let target = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let refine_dir = test_refine_dir(&target_root);
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Refresh candidate", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        if let Some(attempt) = automatic_retry_attempt {
            work_items
                .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
                .unwrap();
            work_items
                .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
                .unwrap();
            work_items
                .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
                .unwrap();
            // Historical lineage remains readable; new recovery requires an explicit decision.
            let goal = work_items.show_goal_detail("GOAL1").unwrap();
            work_items
                .control_workflow(
                    "GOAL1",
                    &crate::application::work_items::WorkflowControl {
                        to: GoalStatus::Plan,
                        reason: format!("Explicit recovery after historical attempt {attempt}"),
                        context: "Continue the recovery lineage".into(),
                        expected_revision: goal["workflow_revision"].as_u64().unwrap_or(0),
                        request_id: "fixture-recovery".into(),
                        force: false,
                        actor: "Operator".into(),
                        invocation_id: None,
                    },
                )
                .unwrap();
        }
        let branch = if automatic_retry_attempt.is_some() {
            "refine/GOAL1/round-2"
        } else {
            "refine/GOAL1/round-1"
        };
        let worktree = if automatic_retry_attempt.is_some() {
            let service = FileGitWorktreeService::new(&target_root);
            let target = service.managed_worktree_path(branch).unwrap();
            PathBuf::from(
                service
                    .ensure_worktree_at_commit(branch, &target, &candidate)
                    .unwrap(),
            )
        } else {
            worktree
        };
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs("GOAL1", branch, "main", &base, Some(&candidate))
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
            .unwrap();
        let (round_idx, revision, request) = work_items.authored_goal_commitment("GOAL1").unwrap();
        let authority = work_items
            .claim_workflow_attempt(
                "GOAL1",
                GoalStatus::Governance,
                round_idx,
                revision,
                &request,
            )
            .unwrap();
        Self {
            temp_root,
            target_root,
            runtime_root,
            worktree,
            work_items,
            base,
            candidate,
            target,
            authority,
        }
    }

    fn context(&self) -> WorkflowContext<'_> {
        let mut context = WorkflowContext::new(
            &self.runtime_root,
            &self.target_root,
            "GOAL1".to_string(),
            "default".to_string(),
            "smoke-ai".to_string(),
            self.authority.round_idx,
            self.authority,
            Default::default(),
            self.work_items.clone(),
        );
        context.branch = self
            .work_items
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .branch_name;
        context.worktree_path = Some(self.worktree.display().to_string());
        context.commit = Some(self.candidate.clone());
        context
    }
}

impl Drop for RefreshFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

fn attempt_concurrent_refresh(
    target_root: PathBuf,
    runtime_root: PathBuf,
    worktree: PathBuf,
    work_items: FileWorkItemService,
    candidate: String,
    authority: WorkflowAttemptAuthority,
    barrier: std::sync::Arc<std::sync::Barrier>,
) -> RefineResult<CandidateRefreshOutcome> {
    let mut context = WorkflowContext::new(
        &runtime_root,
        &target_root,
        "GOAL1".to_string(),
        "default".to_string(),
        "smoke-ai".to_string(),
        0,
        authority,
        Default::default(),
        work_items,
    );
    context.branch = Some("refine/GOAL1/round-1".to_string());
    context.worktree_path = Some(worktree.display().to_string());
    context.commit = Some(candidate);
    barrier.wait();
    refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance)
}

#[test]
fn target_advancement_rebases_a_provable_delta_and_records_both_identities() {
    let fixture = RefreshFixture::new(false);
    let mut context = fixture.context();
    let outcome =
        refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance).unwrap();
    let CandidateRefreshOutcome::Refreshed {
        replacement_candidate,
        ..
    } = outcome
    else {
        panic!("expected candidate refresh")
    };
    assert_ne!(replacement_candidate, fixture.candidate);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["base_commit"], fixture.target);
    assert_eq!(detail["candidate_commit"], replacement_candidate);
    assert_eq!(
        detail["rounds"][0]["workflow_candidate_refresh"]["original_base_commit"],
        fixture.base
    );
    assert_eq!(
        detail["rounds"][0]["workflow_candidate_refresh"]["original_candidate_commit"],
        fixture.candidate
    );
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        replacement_candidate
    );
    assert!(
        !git_output(
            &fixture.target_root,
            &["cat-file", "-t", &fixture.candidate]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn repository_lease_serializes_competing_target_refreshes() {
    let fixture = RefreshFixture::new(false);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let spawn = |barrier: std::sync::Arc<std::sync::Barrier>| {
        let target_root = fixture.target_root.clone();
        let runtime_root = fixture.runtime_root.clone();
        let worktree = fixture.worktree.clone();
        let work_items = fixture.work_items.clone();
        let candidate = fixture.candidate.clone();
        let authority = fixture.authority;
        std::thread::spawn(move || {
            attempt_concurrent_refresh(
                target_root,
                runtime_root,
                worktree,
                work_items,
                candidate,
                authority,
                barrier,
            )
        })
    };
    let first = spawn(barrier.clone());
    let second = spawn(barrier);
    let outcomes = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Ok(CandidateRefreshOutcome::Refreshed { .. })))
            .count(),
        1
    );
    let rejected = outcomes
        .iter()
        .find_map(|outcome| outcome.as_ref().err())
        .unwrap();
    assert!(
        rejected
            .to_string()
            .contains("hydrated candidate identity changed")
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    let replacement = detail["candidate_commit"].as_str().unwrap();
    assert_ne!(replacement, fixture.candidate);
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["rounds"][0]["workflow_candidate_refresh"]["original_candidate_commit"],
        fixture.candidate
    );
    assert_eq!(
        git_output(&fixture.worktree, &["status", "--porcelain"]),
        ""
    );
}

#[test]
fn authority_loss_after_rebase_restores_the_original_candidate_branch() {
    let fixture = RefreshFixture::new(false);
    let mut context = fixture.context();
    let worktree_git =
        FileGitWorktreeService::with_runtime_root(&fixture.worktree, &fixture.runtime_root);
    let rebase = worktree_git.rebase("main").unwrap();
    assert!(rebase.ok, "{rebase:?}");
    let replacement = git_output(&fixture.worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    assert_ne!(replacement, fixture.candidate);
    fixture.work_items.cancel_goal_summary("GOAL1").unwrap();

    let error = crate::application::workflow::recovery::candidate_refresh::persist_candidate_refresh_or_restore(
        &mut context,
        &GoalStatus::Governance,
        &worktree_git,
        "refine/GOAL1/round-1",
        fixture.worktree.to_str().unwrap(),
        &fixture.base,
        &fixture.candidate,
        &fixture.target,
        &replacement,
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no longer authorizes governance work"),
        "{error}"
    );
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    assert_eq!(
        fixture.work_items.show_goal_detail("GOAL1").unwrap()["candidate_commit"],
        fixture.candidate
    );
}

#[test]
fn refresh_conflict_aborts_and_retains_evidence_without_another_round() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    // Resolver disabled (`workflow_conflict_resolution` off): exactly the
    // pre-resolver behavior.
    let outcome =
        refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance).unwrap();
    assert!(matches!(outcome, CandidateRefreshOutcome::Stopped { .. }));
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["reason"],
        "candidate refresh conflicted"
    );
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["original_candidate_commit"],
        fixture.candidate
    );
    assert!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["rebase"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
}

#[test]
fn refresh_conflict_after_explicit_recovery_has_no_successor_round() {
    let fixture = RefreshFixture::new_with_retry(true, Some(5));
    let mut context = fixture.context();
    let outcome =
        refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance).unwrap();
    assert!(matches!(outcome, CandidateRefreshOutcome::Stopped { .. }));
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(detail["rounds"][1]["workflow_recovery"]["state"], "failed");
    assert!(detail["rounds"][1]["workflow_recovery"]["successor_round"].is_null());
    assert_eq!(detail["rounds"][1]["failure_category"], "integration");
    assert_eq!(
        detail["rounds"][1]["workflow_recovery"]["retained_evidence"]["original_candidate_commit"],
        fixture.candidate
    );
}

#[test]
fn integration_merge_conflict_retains_paths_without_another_round() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let outcome =
        crate::application::workflow::engine::behaviors::settle_integration_merge_conflict(
            &mut context,
            vec!["app.txt".to_string()],
            "candidate integration failed: CONFLICT (content): Merge conflict in app.txt"
                .to_string(),
        )
        .unwrap();
    let crate::application::workflow::engine::behaviors::contract::WorkflowAdvanceOutcome::Completed {
        final_status,
        ..
    } = outcome
    else {
        panic!("expected a completed outcome");
    };
    assert_eq!(final_status, GoalStatus::Failed);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["reason"],
        "candidate integration merge conflicted"
    );
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
    // The conflicted paths survive into the recovery Round evidence exactly
    // as the rebase path retains `rebase.conflicts`.
    assert!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["merge"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
}

#[test]
fn integration_merge_conflict_after_explicit_recovery_fails_once() {
    let fixture = RefreshFixture::new_with_retry(true, Some(5));
    let mut context = fixture.context();
    let outcome =
        crate::application::workflow::engine::behaviors::settle_integration_merge_conflict(
            &mut context,
            vec!["app.txt".to_string()],
            "candidate integration failed: CONFLICT (content): Merge conflict in app.txt"
                .to_string(),
        )
        .unwrap();
    let crate::application::workflow::engine::behaviors::contract::WorkflowAdvanceOutcome::Completed {
        final_status,
        ..
    } = outcome
    else {
        panic!("expected a completed outcome");
    };
    assert_eq!(final_status, GoalStatus::Failed);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(detail["rounds"][1]["workflow_recovery"]["state"], "failed");
    assert_eq!(detail["rounds"][1]["failure_category"], "integration");
    assert!(
        detail["rounds"][1]["workflow_recovery"]["retained_evidence"]["merge"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
}

#[test]
fn an_explicit_recovery_failure_does_not_create_another_round() {
    let fixture = RefreshFixture::new_with_retry(true, Some(4));
    let mut context = fixture.context();
    let outcome =
        refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance).unwrap();
    assert!(matches!(outcome, CandidateRefreshOutcome::Stopped { .. }));
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    // The explicitly requested Round remains the last Round after failure.
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
    assert!(
        detail["rounds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|round| round["automatic_retry"].is_null())
    );
}

/// Crash-only: an interruption during unlocked resolution leaves the rebase
/// stopped mid-pick. The next attempt rejects that checkout on provenance —
/// and must abort the rebase it found rather than hand the recovery Round a
/// wedged worktree.
#[test]
fn an_interrupted_rebase_is_aborted_and_retained_without_a_new_round() {
    let fixture = RefreshFixture::new(true);
    // Exactly what a crash between the two holds leaves behind.
    assert!(git(&fixture.worktree, &["rebase", "main"]).is_err());
    let worktree_git =
        FileGitWorktreeService::with_runtime_root(&fixture.worktree, &fixture.runtime_root);
    assert!(worktree_git.operation_in_progress().unwrap());
    let mut context = fixture.context();

    let outcome =
        refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance).unwrap();

    let CandidateRefreshOutcome::Stopped { evidence, .. } = outcome else {
        panic!("expected the recovery fallback");
    };
    assert!(!worktree_git.operation_in_progress().unwrap());
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate,
        "aborting restores the exact recorded candidate"
    );
    assert_eq!(
        evidence["restored_candidate_head"]["commit"], fixture.candidate,
        "{evidence:#}"
    );
    assert!(!evidence["rebase_abort"].is_null(), "{evidence:#}");
}
