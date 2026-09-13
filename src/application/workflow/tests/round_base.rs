//! Where a Round's base comes from, and what happens when the candidate does
//! not descend from it.
//!
//! Every other workflow fixture initializes its repository on `main` and never
//! leaves it, which makes the shared checkout's HEAD and the configured merge
//! target the same commit by construction — the one arrangement in which
//! creating a Round branch from HEAD looks correct. These tests park the shared
//! checkout on a *different* branch first, which is the ordinary state of a
//! repository a human also works in, and the state in which a branch born at
//! HEAD silently descends from the wrong place.

use super::*;

use crate::application::work_items::WorkflowStepAuthority;
use crate::application::workflow::engine::behaviors::contract::{
    WorkflowAdvanceOutcome, WorkflowBehavior,
};
use crate::application::workflow::engine::behaviors::{WorkflowTodo, settle_stale_candidate};
use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::engine::context::execution::hydrate_plan_or_implement_context;
use crate::application::workflow::governance::integration::{
    TargetRefresh, refresh_target_from_remote,
};
use crate::infrastructure::git::worktrees::FileGitWorktreeService;
use crate::model::JsonObject;

/// A repository whose shared checkout sits on `develop` while the configured
/// merge target is `main`, with a Goal ready to leave Todo.
struct CheckoutElsewhereFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    work_items: FileWorkItemService,
    /// Tip of the configured merge target.
    main_tip: String,
    /// Tip of the unrelated branch the human left checked out.
    develop_tip: String,
}

impl CheckoutElsewhereFixture {
    fn new(prefix: &str) -> Self {
        let temp_root = unique_temp_dir(prefix);
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
        fs::write(target_root.join("app.txt"), "target\n").unwrap();
        git(&target_root, &["add", "app.txt"]).unwrap();
        git(&target_root, &["commit", "-q", "-m", "target base"]).unwrap();
        let main_tip = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();

        // The human's branch, carrying a commit the merge target does not have,
        // left checked out exactly as a working repository is found.
        git(&target_root, &["checkout", "-q", "-b", "develop"]).unwrap();
        fs::write(target_root.join("unrelated.txt"), "unrelated work\n").unwrap();
        git(&target_root, &["add", "unrelated.txt"]).unwrap();
        git(&target_root, &["commit", "-q", "-m", "unrelated work"]).unwrap();
        let develop_tip = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        assert_ne!(main_tip, develop_tip);

        let refine_dir = test_refine_dir(&target_root);
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Round base", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();

        Self {
            temp_root,
            target_root,
            runtime_root,
            work_items,
            main_tip,
            develop_tip,
        }
    }

    fn settings(&self) -> JsonObject {
        let mut settings = JsonObject::new();
        settings.insert("merge_target_branch".to_string(), json!("main"));
        settings.insert("branch_name_pattern".to_string(), json!("refine/{goal_id}"));
        settings
    }

    fn authority(&self, status: GoalStatus) -> WorkflowStepAuthority {
        let (round_idx, revision, request) =
            self.work_items.authored_goal_commitment("GOAL1").unwrap();
        self.work_items
            .claim_workflow_attempt("GOAL1", status, round_idx, revision, &request)
            .unwrap()
    }

    fn context(&self, authority: WorkflowStepAuthority) -> WorkflowContext<'_> {
        WorkflowContext::new(
            &self.runtime_root,
            &self.target_root,
            "GOAL1".to_string(),
            "default".to_string(),
            "smoke-ai".to_string(),
            authority.round_idx,
            authority,
            self.settings(),
            self.work_items.clone(),
        )
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(&self.target_root)
            .output()
            .unwrap()
            .status
            .success()
    }
}

impl Drop for CheckoutElsewhereFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

#[test]
fn round_branch_is_born_at_the_merge_target_not_at_the_shared_checkout_head() {
    let fixture = CheckoutElsewhereFixture::new("round-base-birth");
    let authority = fixture.authority(GoalStatus::Todo);
    let mut ctx = fixture.context(authority);

    let outcome = WorkflowTodo.advance(&mut ctx).unwrap();
    assert!(
        matches!(
            outcome,
            WorkflowAdvanceOutcome::Transition {
                to: GoalStatus::Plan,
                ..
            }
        ),
        "expected admission to Plan, got {outcome:?}"
    );

    let branch_tip = git_output(&fixture.target_root, &["rev-parse", "refine/GOAL1/round-1"])
        .trim()
        .to_string();
    assert_eq!(
        branch_tip, fixture.main_tip,
        "the Round branch must start at the configured merge target"
    );

    // The property the integration gate actually checks.
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    let recorded_base = detail["base_commit"].as_str().unwrap();
    assert_eq!(recorded_base, fixture.main_tip);
    assert!(
        fixture.is_ancestor(recorded_base, &branch_tip),
        "the recorded base must be an ancestor of the Round branch by construction"
    );
    assert!(
        !fixture.is_ancestor(&fixture.develop_tip, &branch_tip),
        "the Round branch must not inherit the human checkout's branch"
    );
}

#[test]
fn resumption_recreates_a_missing_round_branch_at_the_recorded_base() {
    let fixture = CheckoutElsewhereFixture::new("round-base-resume");
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    fixture
        .work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &fixture.main_tip,
            None,
        )
        .unwrap();
    let authority = fixture.authority(GoalStatus::Plan);
    let mut ctx = fixture.context(authority);

    // The branch was never created (or was cleaned up externally): resumption
    // must rebuild it from the Goal's own record, not from the human checkout.
    hydrate_plan_or_implement_context(&mut ctx, "refine/{goal_id}", "main").unwrap();

    let branch_tip = git_output(&fixture.target_root, &["rev-parse", "refine/GOAL1/round-1"])
        .trim()
        .to_string();
    assert_eq!(branch_tip, fixture.main_tip);
    assert!(!fixture.is_ancestor(&fixture.develop_tip, &branch_tip));
}

#[test]
fn resumption_reuses_an_existing_round_branch_exactly_as_it_stands() {
    let fixture = CheckoutElsewhereFixture::new("round-base-reuse");
    let worktree = fixture
        .target_root
        .join(".git/refine-worktrees/refine-GOAL1-round-1");
    fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &fixture.target_root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "refine/GOAL1/round-1",
            worktree.to_str().unwrap(),
            &fixture.main_tip,
        ],
    )
    .unwrap();
    fs::write(worktree.join("progress.txt"), "interrupted work\n").unwrap();
    git(&worktree, &["add", "progress.txt"]).unwrap();
    git(&worktree, &["commit", "-q", "-m", "interrupted work"]).unwrap();
    let interrupted_tip = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();

    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    fixture
        .work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &fixture.main_tip,
            None,
        )
        .unwrap();
    let authority = fixture.authority(GoalStatus::Implement);
    let mut ctx = fixture.context(authority);

    hydrate_plan_or_implement_context(&mut ctx, "refine/{goal_id}", "main").unwrap();

    assert_eq!(
        git_output(&fixture.target_root, &["rev-parse", "refine/GOAL1/round-1"]).trim(),
        interrupted_tip,
        "an interrupted Round's commits must survive resumption untouched"
    );
}

/// A repository whose remote target branch has moved ahead of the local ref,
/// which is the ordinary state of a checkout nobody pulls.
struct StaleLocalTargetFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    remote_tip: String,
    local_tip: String,
}

impl StaleLocalTargetFixture {
    fn new(prefix: &str) -> Self {
        let temp_root = unique_temp_dir(prefix);
        let target_root = temp_root.join("repo");
        let remote = temp_root.join("remote.git");
        let publisher = temp_root.join("publisher");
        fs::create_dir_all(&target_root).unwrap();
        git(&target_root, &["init", "-b", "main"]).unwrap();
        git(
            &target_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
        fs::write(target_root.join("app.txt"), "one\n").unwrap();
        git(&target_root, &["add", "app.txt"]).unwrap();
        git(&target_root, &["commit", "-q", "-m", "one"]).unwrap();
        git(
            &target_root,
            &[
                "init",
                "--bare",
                "-q",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(
            &target_root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&target_root, &["push", "-q", "origin", "main"]).unwrap();
        let local_tip = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();

        // Someone else advances the shared branch.
        git(
            &target_root,
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                publisher.to_str().unwrap(),
            ],
        )
        .unwrap();
        git(
            &publisher,
            &["config", "user.email", "other@example.invalid"],
        )
        .unwrap();
        git(&publisher, &["config", "user.name", "Other"]).unwrap();
        fs::write(publisher.join("app.txt"), "two\n").unwrap();
        git(&publisher, &["commit", "-q", "-am", "two"]).unwrap();
        git(&publisher, &["push", "-q", "origin", "main"]).unwrap();
        let remote_tip = git_output(&publisher, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        assert_ne!(local_tip, remote_tip);

        test_refine_dir(&target_root);
        Self {
            temp_root,
            target_root,
            remote_tip,
            local_tip,
        }
    }

    fn git(&self) -> FileGitWorktreeService {
        FileGitWorktreeService::with_runtime_root(
            &self.target_root,
            self.temp_root.join("run/8080"),
        )
    }
}

impl Drop for StaleLocalTargetFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

/// The preparation fast-forward runs outside the integrated-target transaction, so its
/// only durable trace of an unfinished checkout sync is the pending record.
#[test]
fn a_dirty_checkout_advances_the_ref_and_leaves_the_sync_pending_for_repair() {
    let fixture = StaleLocalTargetFixture::new("target-refresh-dirty");
    let pending_marker = fixture
        .target_root
        .join(".git/refine-checkout-sync-pending.json");
    // A human edit to the very file the advance changes: Git refuses to
    // overwrite it, so the sync is skipped rather than silently discarding work.
    fs::write(
        fixture.target_root.join("app.txt"),
        "uncommitted human edit\n",
    )
    .unwrap();

    let refresh =
        refresh_target_from_remote(&fixture.git(), &fixture.target_root, "origin", "main");

    assert!(
        matches!(refresh, TargetRefresh::FastForwarded { .. }),
        "a collision must not fail the refresh, got {refresh:?}"
    );
    assert_eq!(
        git_output(&fixture.target_root, &["rev-parse", "main"]).trim(),
        fixture.remote_tip
    );
    assert_eq!(
        fs::read_to_string(fixture.target_root.join("app.txt")).unwrap(),
        "uncommitted human edit\n",
        "the human's uncommitted edit must survive"
    );
    assert!(
        pending_marker.exists(),
        "a skipped sync must leave a durable record to repair from"
    );

    // Once the colliding edit is gone, the next Round's refresh repairs the
    // checkout on its way past — no separate repair pass is needed.
    git(&fixture.target_root, &["checkout", "--", "app.txt"]).unwrap();
    let refresh =
        refresh_target_from_remote(&fixture.git(), &fixture.target_root, "origin", "main");

    assert!(matches!(refresh, TargetRefresh::AlreadyCurrent { .. }));
    assert_eq!(
        fs::read_to_string(fixture.target_root.join("app.txt")).unwrap(),
        "two\n"
    );
    assert!(
        !pending_marker.exists(),
        "the repaired record must be cleared"
    );
}

#[test]
fn a_stale_local_target_fast_forwards_from_its_remote_before_the_base_is_pinned() {
    let fixture = StaleLocalTargetFixture::new("target-refresh-ff");
    let git_service = fixture.git();

    let refresh = refresh_target_from_remote(&git_service, &fixture.target_root, "origin", "main");

    let TargetRefresh::FastForwarded {
        from_commit,
        to_commit,
    } = &refresh
    else {
        panic!("expected a fast-forward, got {refresh:?}")
    };
    assert_eq!(from_commit, &fixture.local_tip);
    assert_eq!(to_commit, &fixture.remote_tip);
    assert_eq!(
        git_output(&fixture.target_root, &["rev-parse", "main"]).trim(),
        fixture.remote_tip,
        "the base is pinned from the local ref, so the local ref must carry the fetched tip"
    );
    // The shared checkout sits on the target branch, so the advance is mirrored
    // into its working tree rather than left as a staged-reverse surprise.
    assert_eq!(
        fs::read_to_string(fixture.target_root.join("app.txt")).unwrap(),
        "two\n"
    );
}

#[test]
fn a_diverged_local_target_is_left_for_integration_to_merge() {
    let fixture = StaleLocalTargetFixture::new("target-refresh-diverged");
    // A local commit the remote does not carry turns the fast-forward into a merge.
    fs::write(fixture.target_root.join("local.txt"), "local only\n").unwrap();
    git(&fixture.target_root, &["add", "local.txt"]).unwrap();
    git(&fixture.target_root, &["commit", "-q", "-m", "local only"]).unwrap();
    let diverged_tip = git_output(&fixture.target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();

    let refresh =
        refresh_target_from_remote(&fixture.git(), &fixture.target_root, "origin", "main");

    assert!(
        matches!(refresh, TargetRefresh::Diverged { .. }),
        "expected divergence to be left alone, got {refresh:?}"
    );
    assert_eq!(
        git_output(&fixture.target_root, &["rev-parse", "main"]).trim(),
        diverged_tip,
        "Preparation must not merge; integration owns that with its own evidence"
    );
}

#[test]
fn an_unreachable_remote_leaves_the_round_to_start_from_the_local_ref() {
    let fixture = StaleLocalTargetFixture::new("target-refresh-unavailable");

    let refresh = refresh_target_from_remote(
        &fixture.git(),
        &fixture.target_root,
        "no-such-remote",
        "main",
    );

    assert!(
        matches!(refresh, TargetRefresh::Unavailable { .. }),
        "a missing remote must degrade, not fail, got {refresh:?}"
    );
    assert_eq!(
        git_output(&fixture.target_root, &["rev-parse", "main"]).trim(),
        fixture.local_tip
    );
}

#[test]
fn integration_target_advancement_retains_failure_in_the_original_round() {
    let fixture = CheckoutElsewhereFixture::new("stale-candidate-race");
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
        .unwrap();
    let authority = fixture.authority(GoalStatus::Governance);
    let mut ctx = fixture.context(authority);

    // The integration guard retains its actual failure. A later workflow
    // decision uses shared preparation without inventing another authored Round.
    let outcome = settle_stale_candidate(
        &mut ctx,
        RefineError::StaleCandidate {
            candidate_commit: fixture.develop_tip.clone(),
            recorded_base: fixture.main_tip.clone(),
            target_branch: "main".to_string(),
            target_commit: fixture.develop_tip.clone(),
        },
    )
    .unwrap();

    assert!(matches!(
        outcome,
        WorkflowAdvanceOutcome::Completed {
            final_status: GoalStatus::Failed,
            ..
        }
    ));
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"][0]["workflow_recovery"]["state"], "failed");
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["kind"],
        "integration"
    );
}

#[test]
fn integration_ancestry_guard_preserves_the_original_diagnosis() {
    let fixture = CheckoutElsewhereFixture::new("stale-candidate-lineage");
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    fixture
        .work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
        .unwrap();
    let authority = fixture.authority(GoalStatus::Governance);
    let mut ctx = fixture.context(authority);

    // The direct integration boundary must reject contradictory provenance.
    // Workspace preparation, exercised below, owns regeneration before a new attempt.
    let error = settle_stale_candidate(
        &mut ctx,
        RefineError::StaleCandidate {
            candidate_commit: fixture.develop_tip.clone(),
            recorded_base: fixture.main_tip.clone(),
            target_branch: "main".to_string(),
            target_commit: fixture.main_tip.clone(),
        },
    )
    .unwrap_err();

    assert!(
        matches!(error, RefineError::StaleCandidate { .. }),
        "the original diagnosis must survive, got {error}"
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(
        detail["rounds"].as_array().unwrap().len(),
        1,
        "a futile retry must not append a recovery Round"
    );
    assert_eq!(
        detail["rounds"][0]["failure_category"],
        "governance_candidate_lineage"
    );
}

#[test]
fn todo_reassignment_preserves_the_base_of_retained_work_after_main_advances() {
    let fixture = CheckoutElsewhereFixture::new("retained-base-after-target-advance");
    let mut ctx = fixture.context(fixture.authority(GoalStatus::Todo));
    WorkflowTodo.advance(&mut ctx).unwrap();
    let branch = ctx.branch.clone().unwrap();
    let workspace = PathBuf::from(ctx.worktree_path.clone().unwrap());
    fs::write(workspace.join("feature.txt"), "retained implementation\n").unwrap();
    git(&workspace, &["add", "feature.txt"]).unwrap();
    git(&workspace, &["commit", "-qm", "implementation"]).unwrap();
    let candidate = git_output(&workspace, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    fixture
        .work_items
        .update_goal_candidate_commit("GOAL1", &candidate)
        .unwrap();
    git(&fixture.target_root, &["checkout", "-q", "main"]).unwrap();
    fs::write(fixture.target_root.join("other.txt"), "published work\n").unwrap();
    git(&fixture.target_root, &["add", "other.txt"]).unwrap();
    git(
        &fixture.target_root,
        &["commit", "-qm", "target advancement"],
    )
    .unwrap();
    fixture
        .work_items
        .override_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    let mut resumed = fixture.context(fixture.authority(GoalStatus::Todo));
    WorkflowTodo.advance(&mut resumed).unwrap();
    let goal = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["base_commit"], fixture.main_tip);
    assert_eq!(goal["branch_name"], branch);
    assert_eq!(goal["candidate_commit"], candidate);
    assert!(fixture.is_ancestor(goal["base_commit"].as_str().unwrap(), &candidate));
}

#[test]
fn corrupt_generated_lineage_restarts_same_authored_round_without_resetting_old_work() {
    let fixture = CheckoutElsewhereFixture::new("regenerate-corrupt-lineage");
    let mut ctx = fixture.context(fixture.authority(GoalStatus::Todo));
    WorkflowTodo.advance(&mut ctx).unwrap();
    let old_branch = ctx.branch.clone().unwrap();
    let workspace = PathBuf::from(ctx.worktree_path.clone().unwrap());
    fs::write(workspace.join("feature.txt"), "retained work\n").unwrap();
    git(&workspace, &["add", "feature.txt"]).unwrap();
    git(&workspace, &["commit", "-qm", "implementation"]).unwrap();
    let candidate = git_output(&workspace, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    // The reported incident: a newer unrelated base is attached to the old branch.
    fixture
        .work_items
        .update_goal_git_refs(
            "GOAL1",
            &old_branch,
            "main",
            &fixture.develop_tip,
            Some(&candidate),
        )
        .unwrap();
    fixture
        .work_items
        .override_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    let mut resumed = fixture.context(fixture.authority(GoalStatus::Todo));
    WorkflowTodo.advance(&mut resumed).unwrap();
    let goal = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_ne!(goal["branch_name"], old_branch);
    assert_eq!(goal["base_commit"], fixture.main_tip);
    assert!(goal["candidate_commit"].is_null());
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(goal["rounds"][0]["prompt"], "Prompt");
    assert_eq!(
        goal["rounds"][0]["workspace_recoveries"][0]["candidate"],
        candidate
    );
    assert_eq!(
        git_output(&workspace, &["rev-parse", "HEAD"]).trim(),
        candidate
    );
    // A replacement daemon reuses the already recorded clean binding.
    let branch = goal["branch_name"].clone();
    let mut replacement = fixture.context(fixture.authority(GoalStatus::Plan));
    hydrate_plan_or_implement_context(&mut replacement, "refine/{goal_id}", "main").unwrap();
    assert_eq!(
        fixture.work_items.show_goal_detail("GOAL1").unwrap()["branch_name"],
        branch
    );
}

#[test]
fn selecting_quality_without_generated_inputs_recovers_through_plan_in_the_same_round() {
    let fixture = CheckoutElsewhereFixture::new("direct-quality-regeneration");
    fixture
        .work_items
        .override_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    let mut ctx = fixture.context(fixture.authority(GoalStatus::Quality));
    crate::application::workflow::engine::context::execution::hydrate_retry_context(
        &mut ctx,
        GoalStatus::Quality,
    )
    .unwrap();
    let goal = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "plan");
    assert_eq!(ctx.start_status, GoalStatus::Plan);
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(goal["rounds"][0]["prompt"], "Prompt");
    assert_eq!(goal["base_commit"], fixture.main_tip);
    assert!(PathBuf::from(ctx.worktree_path.unwrap()).exists());
}

#[test]
fn sibling_candidate_and_branch_require_regeneration_even_with_a_shared_base() {
    let fixture = CheckoutElsewhereFixture::new("divergent-candidate-branch");
    let mut ctx = fixture.context(fixture.authority(GoalStatus::Todo));
    WorkflowTodo.advance(&mut ctx).unwrap();
    let branch = ctx.branch.clone().unwrap();
    let workspace = PathBuf::from(ctx.worktree_path.clone().unwrap());
    fs::write(workspace.join("feature.txt"), "branch-only work\n").unwrap();
    git(&workspace, &["add", "feature.txt"]).unwrap();
    git(&workspace, &["commit", "-qm", "branch work"]).unwrap();
    let old_tip = git_output(&workspace, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    assert!(fixture.is_ancestor(&fixture.main_tip, &old_tip));
    assert!(fixture.is_ancestor(&fixture.main_tip, &fixture.develop_tip));
    assert!(!fixture.is_ancestor(&fixture.develop_tip, &old_tip));
    fixture
        .work_items
        .update_goal_candidate_commit("GOAL1", &fixture.develop_tip)
        .unwrap();
    fixture
        .work_items
        .override_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    let mut resumed = fixture.context(fixture.authority(GoalStatus::Quality));
    crate::application::workflow::engine::context::execution::hydrate_retry_context(
        &mut resumed,
        GoalStatus::Quality,
    )
    .unwrap();
    let goal = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "plan");
    assert_ne!(goal["branch_name"], branch);
    assert!(goal["candidate_commit"].is_null());
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(
        git_output(&workspace, &["rev-parse", "HEAD"]).trim(),
        old_tip
    );
}

#[test]
fn retained_legacy_seed_uses_common_preparation_with_its_actual_base() {
    let fixture = CheckoutElsewhereFixture::new("legacy-retained-seed");
    fixture
        .work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &fixture.main_tip,
            Some(&fixture.develop_tip),
        )
        .unwrap();
    fixture
        .work_items
        .override_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    fixture
        .work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Continue retained work")
        .unwrap();
    // Seed the retired persisted schema directly; the current evaluation API
    // intentionally cannot manufacture automatic retry records.
    let summary = fixture.work_items.show_goal_summary("GOAL1").unwrap();
    let path = fixture.work_items.refine_dir.join(&summary.goal.json_path);
    let mut historical: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    historical["rounds"][1]["automatic_retry"] = json!({"kind":"quality","source_round":1});
    historical["rounds"][1]["retained_candidate"] = json!(fixture.develop_tip);
    fs::write(&path, serde_json::to_vec_pretty(&historical).unwrap()).unwrap();
    let mut ctx = fixture.context(fixture.authority(GoalStatus::Todo));
    WorkflowTodo.advance(&mut ctx).unwrap();
    let goal = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(goal["branch_name"], "refine/GOAL1/round-2");
    assert_eq!(goal["base_commit"], fixture.main_tip);
    assert_eq!(goal["candidate_commit"], fixture.develop_tip);
    assert!(goal["rounds"][1]["workspace_recoveries"].is_null());
    assert_eq!(
        git_output(
            &PathBuf::from(ctx.worktree_path.unwrap()),
            &["rev-parse", "HEAD"]
        )
        .trim(),
        fixture.develop_tip
    );
}
