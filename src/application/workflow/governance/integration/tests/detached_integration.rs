use super::*;
use crate::application::workflow::governance::integration::checkout_sync::{
    CheckoutSyncOutcome, repair_pending_checkout_sync, sync_human_checkout_after_ref_move,
};

const GOAL: &str = "GOAL1";
const BRANCH: &str = "refine/GOAL1/round-1";
const MARKER: &str = ".git/refine-checkout-sync-pending.json";

struct DetachedIntegrationFixture {
    temp_root: PathBuf,
    repo: PathBuf,
    runtime_root: PathBuf,
    worktree_path: PathBuf,
    base_commit: String,
    candidate_commit: String,
    work_items: FileWorkItemService,
    service: FileGovernanceIntegrationService,
}

impl DetachedIntegrationFixture {
    /// Temp repo with a bare `origin`, a pushed candidate branch built by
    /// `candidate`, and a Goal in governance authorizing its integration.
    fn new(name: &str, candidate: impl FnOnce(&Path)) -> Self {
        let temp_root = unique_temp_dir(name);
        let repo = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("candidate");
        let remote = temp_root.join("remote.git");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let refine_dir = prepare_refine_dir(&repo).unwrap();
        commit_file(&repo, "app.txt", "base\n", "initial");
        let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-u", "origin", "main"]).unwrap();
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                BRANCH,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        candidate(&worktree_path);
        let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);
        git(&worktree_path, &["push", "-u", "origin", BRANCH]).unwrap();

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items.create_goal_summary(GOAL, Some(GOAL)).unwrap();
        work_items
            .append_goal_round_summary(GOAL, "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status(GOAL, GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs(GOAL, BRANCH, "main", &base_commit, Some(&candidate_commit))
            .unwrap();
        work_items
            .update_goal_round_evaluation_summary(
                GOAL,
                0,
                &json!({"workflow_git_remote": "origin"}),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Quality)
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Governance)
            .unwrap();
        let service = FileGovernanceIntegrationService::new(&runtime_root, &refine_dir);
        Self {
            temp_root,
            repo,
            runtime_root,
            worktree_path,
            base_commit,
            candidate_commit,
            work_items,
            service,
        }
    }

    fn integrate(&self) -> RefineResult<RoundIntegration> {
        self.service.integrate_workflow_candidate(
            GOAL,
            0,
            "default",
            BRANCH,
            &self.candidate_commit,
            "origin",
        )
    }

    fn repo_git(&self) -> FileGitWorktreeService {
        FileGitWorktreeService::with_runtime_root(&self.repo, &self.runtime_root)
    }

    fn integration_worktree(&self) -> PathBuf {
        self.repo.join(".git/refine-integration/target")
    }

    fn assert_no_merge_state_in_human_checkout(&self) {
        assert!(
            !git_succeeds(&self.repo, &["rev-parse", "--verify", "MERGE_HEAD"]),
            "the human checkout must never carry integration merge state"
        );
        let audit_path = self.repo.join(".git/refine-audit.jsonl");
        if audit_path.exists() {
            let audit = fs::read_to_string(&audit_path).unwrap();
            assert!(
                !audit.contains("\"action\":\"merge_commit_no_ff\""),
                "no merge porcelain may run in the human checkout"
            );
        }
    }

    fn finish(self) {
        git(
            &self.repo,
            &[
                "worktree",
                "remove",
                "--force",
                self.worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::remove_dir_all(self.temp_root).unwrap();
    }
}

#[test]
fn detached_integration_merges_with_two_parents_and_syncs_the_checkout() {
    let fixture = DetachedIntegrationFixture::new("detached-clean", |worktree| {
        commit_file(worktree, "feature.txt", "candidate\n", "candidate");
    });
    // Advance the target past the candidate's base so the merge is genuine.
    commit_file(&fixture.repo, "other.txt", "target work\n", "target work");
    git(&fixture.repo, &["push", "origin", "main"]).unwrap();
    let target_before = git_stdout(&fixture.repo, &["rev-parse", "main"]);

    let integration = fixture.integrate().unwrap();
    assert!(integration.merge.ok);
    assert!(integration.pushed);
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        integration.target_commit
    );
    let parents = git_stdout(
        &fixture.repo,
        &[
            "rev-list",
            "--parents",
            "-n",
            "1",
            &integration.target_commit,
        ],
    );
    let parents: Vec<&str> = parents.split_whitespace().skip(1).collect();
    assert_eq!(
        parents,
        vec![target_before.as_str(), fixture.candidate_commit.as_str()],
        "integration must be a two-parent merge with the candidate as parent 2"
    );
    assert!(
        git_stdout(&fixture.repo, &["ls-remote", "origin", "refs/heads/main"])
            .starts_with(&integration.target_commit)
    );
    // The human checkout stayed on main and was synced in place.
    assert_eq!(
        git_stdout(&fixture.repo, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "HEAD"]),
        integration.target_commit
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("feature.txt")).unwrap(),
        "candidate\n"
    );
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());
    assert!(!fixture.repo.join(MARKER).exists());
    fixture.assert_no_merge_state_in_human_checkout();
    fixture.finish();
}

#[test]
fn ff_shaped_candidate_still_integrates_as_a_two_parent_merge() {
    let fixture = DetachedIntegrationFixture::new("detached-ff-shape", |worktree| {
        commit_file(worktree, "feature.txt", "candidate\n", "candidate");
    });
    // The target never moves, so the candidate could fast-forward; the
    // already-merged revert machinery still requires a two-parent merge.
    let integration = fixture.integrate().unwrap();
    let parents = git_stdout(
        &fixture.repo,
        &[
            "rev-list",
            "--parents",
            "-n",
            "1",
            &integration.target_commit,
        ],
    );
    let parents: Vec<&str> = parents.split_whitespace().skip(1).collect();
    assert_eq!(
        parents,
        vec![
            fixture.base_commit.as_str(),
            fixture.candidate_commit.as_str()
        ]
    );
    fixture.assert_no_merge_state_in_human_checkout();
    fixture.finish();
}

#[test]
fn conflicted_candidate_leaves_target_checkout_and_worktree_recovered() {
    let fixture = DetachedIntegrationFixture::new("detached-conflict", |worktree| {
        commit_file(worktree, "app.txt", "candidate\n", "candidate");
    });
    commit_file(&fixture.repo, "app.txt", "target\n", "target");
    git(&fixture.repo, &["push", "origin", "main"]).unwrap();
    let target_before = git_stdout(&fixture.repo, &["rev-parse", "main"]);

    let error = fixture.integrate().unwrap_err();
    // The conflict is a typed workflow signal, not a terminal failure: the
    // conflicted paths must survive for the recovery Round's evidence.
    let RefineError::MergeConflict {
        stage, conflicts, ..
    } = &error
    else {
        panic!("expected a typed merge conflict, got {error}");
    };
    assert_eq!(*stage, MergeConflictStage::CandidateIntegration);
    assert!(
        conflicts.iter().any(|path| path == "app.txt"),
        "{conflicts:?}"
    );
    assert!(
        error.to_string().contains("candidate integration failed"),
        "{error}"
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        target_before,
        "a conflicted merge must not move the target ref"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("app.txt")).unwrap(),
        "target\n"
    );
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());
    fixture.assert_no_merge_state_in_human_checkout();
    let worktree = fixture.integration_worktree();
    assert!(worktree.exists());
    assert!(
        git_stdout(&worktree, &["status", "--porcelain"]).is_empty(),
        "the integration worktree must be recovered after a conflict"
    );
    assert!(!git_succeeds(
        &worktree,
        &["rev-parse", "--verify", "MERGE_HEAD"]
    ));
    assert!(
        fixture.work_items.show_goal_detail(GOAL).unwrap()["rounds"][0]["workflow_integration"]
            .is_null()
    );
    fixture.finish();
}

#[test]
fn cas_loss_surfaces_the_target_advanced_retry_signal() {
    let fixture = DetachedIntegrationFixture::new("detached-cas-lost", |worktree| {
        commit_file(worktree, "feature.txt", "candidate\n", "candidate");
    });
    // Prepare an unrelated commit, then move main back so the ref can be
    // yanked to it mid-integration by a post-merge hook: the hook fires after
    // the merge is computed in the integration worktree and before the
    // compare-and-swap publishes it.
    commit_file(&fixture.repo, "outside.txt", "outside\n", "outside");
    let outside = git_stdout(&fixture.repo, &["rev-parse", "HEAD"]);
    git(&fixture.repo, &["reset", "--hard", &fixture.base_commit]).unwrap();
    let hook = fixture.repo.join(".git/hooks/post-merge");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        format!("#!/bin/sh\ngit update-ref refs/heads/main {outside}\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
    }

    let error = fixture.integrate().unwrap_err();
    assert!(
        matches!(error, RefineError::TargetAdvanced { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("target advanced"), "{error}");
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        outside,
        "the lost compare-and-swap must leave the externally moved ref alone"
    );
    assert!(
        fixture.work_items.show_goal_detail(GOAL).unwrap()["rounds"][0]["workflow_integration"]
            .is_null()
    );
    // The checkout sync never ran, so the human checkout still shows the
    // pre-integration content.
    assert!(!fixture.repo.join("feature.txt").exists());
    assert!(!fixture.repo.join(MARKER).exists());
    fixture.finish();
}

#[test]
fn checkout_on_another_branch_stays_byte_identical() {
    let fixture = DetachedIntegrationFixture::new("detached-parked-branch", |worktree| {
        commit_file(worktree, "feature.txt", "candidate\n", "candidate");
    });
    git(&fixture.repo, &["switch", "-c", "parked"]).unwrap();

    let integration = fixture.integrate().unwrap();
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        integration.target_commit
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["branch", "--show-current"]),
        "parked"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("app.txt")).unwrap(),
        "base\n"
    );
    assert!(!fixture.repo.join("feature.txt").exists());
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());
    assert!(
        !fixture.repo.join(MARKER).exists(),
        "a checkout on another branch needs no sync marker"
    );
    fixture.assert_no_merge_state_in_human_checkout();
    // Switching to the target later picks up the integrated tip natively.
    git(&fixture.repo, &["switch", "main"]).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("feature.txt")).unwrap(),
        "candidate\n"
    );
    fixture.finish();
}

#[test]
fn dirty_non_colliding_human_checkout_survives_integration_verbatim() {
    let fixture = DetachedIntegrationFixture::new("detached-dirty-checkout", |worktree| {
        commit_file(worktree, "feature.txt", "candidate\n", "candidate");
    });
    // The production regression: human dirt in the shared checkout that does
    // not collide with the integration delta must neither block the
    // integration nor be touched by it.
    fs::write(fixture.repo.join("scratch.txt"), "untracked human file\n").unwrap();
    fs::write(fixture.repo.join("app.txt"), "human edit in flight\n").unwrap();

    let integration = fixture.integrate().unwrap();
    assert!(integration.pushed);
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "HEAD"]),
        integration.target_commit
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("scratch.txt")).unwrap(),
        "untracked human file\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("app.txt")).unwrap(),
        "human edit in flight\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("feature.txt")).unwrap(),
        "candidate\n"
    );
    let status = git_stdout(&fixture.repo, &["status", "--porcelain"]);
    let mut lines: Vec<&str> = status.lines().map(str::trim).collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        vec!["?? scratch.txt", "M app.txt"],
        "only the human's own dirt may remain after the sync"
    );
    assert!(!fixture.repo.join(MARKER).exists());
    fixture.assert_no_merge_state_in_human_checkout();
    fixture.finish();
}

#[test]
fn colliding_dirt_skips_the_sync_and_repair_clears_the_marker() {
    let fixture = DetachedIntegrationFixture::new("detached-colliding-dirt", |worktree| {
        commit_file(worktree, "app.txt", "candidate\n", "candidate");
        commit_file(worktree, "feature.txt", "candidate\n", "feature");
    });
    fs::write(fixture.repo.join("app.txt"), "human edit in flight\n").unwrap();

    let integration = fixture.integrate().unwrap();
    assert!(integration.pushed);
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        integration.target_commit,
        "a sync collision must not hold back the ref advance"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("app.txt")).unwrap(),
        "human edit in flight\n"
    );
    let marker_path = fixture.repo.join(MARKER);
    let records: Value = serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 1, "one record per target branch");
    let marker = &records[0];
    assert_eq!(marker["reference"], "refs/heads/main");
    assert_eq!(marker["from_commit"], fixture.base_commit);
    assert_eq!(marker["to_commit"], integration.target_commit);
    let reason = marker["reason"].as_str().unwrap();
    assert!(reason.contains("app.txt"), "{reason}");
    assert!(
        reason.contains("the human checkout shows the integration delta as staged-reverse"),
        "{reason}"
    );
    assert!(reason.contains("repair_pending_checkout_sync"), "{reason}");
    // The staged-reverse state the marker describes: the index still holds the
    // pre-integration tree, so the delta shows up as staged deletions.
    assert!(
        git_stdout(&fixture.repo, &["status", "--porcelain"])
            .lines()
            .any(|line| line == "D  feature.txt"),
        "the skipped sync leaves the integration delta staged-reverse"
    );

    // Repair is refused while the collision persists, and the marker stays.
    repair_pending_checkout_sync(&fixture.repo_git(), &fixture.repo).unwrap();
    assert!(marker_path.exists());

    git(&fixture.repo, &["checkout", "--", "app.txt"]).unwrap();
    repair_pending_checkout_sync(&fixture.repo_git(), &fixture.repo).unwrap();
    assert!(!marker_path.exists(), "repair must clear the marker");
    assert_eq!(
        fs::read_to_string(fixture.repo.join("app.txt")).unwrap(),
        "candidate\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("feature.txt")).unwrap(),
        "candidate\n"
    );
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());

    // A repeated repair with no marker is a no-op.
    repair_pending_checkout_sync(&fixture.repo_git(), &fixture.repo).unwrap();
    fixture.finish();
}

#[test]
fn pending_sync_records_for_different_branches_coexist_and_repair_independently() {
    let temp_root = unique_temp_dir("pending-sync-per-branch");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "base");
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "alpha"]).unwrap();
    let repo_git = FileGitWorktreeService::new(&repo);
    let marker_path = repo.join(MARKER);

    // Desync a branch: advance its ref past the checkout, then collide.
    let open_collision = |branch: &str, content: &str| {
        git(&repo, &["switch", branch]).unwrap();
        commit_file(
            &repo,
            "app.txt",
            content,
            &format!("integrated on {branch}"),
        );
        let to = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["reset", "-q", "--hard", &base]).unwrap();
        git(&repo, &["update-ref", &format!("refs/heads/{branch}"), &to]).unwrap();
        fs::write(repo.join("app.txt"), "human edit in flight\n").unwrap();
        let outcome =
            sync_human_checkout_after_ref_move(&repo_git, &repo, branch, &base, &to).unwrap();
        assert!(matches!(
            outcome,
            CheckoutSyncOutcome::SkippedDirtyCollision { .. }
        ));
        git(&repo, &["checkout", "--", "app.txt"]).unwrap();
        to
    };
    let main_to = open_collision("main", "integrated on main\n");
    let alpha_to = open_collision("alpha", "integrated on alpha\n");

    // A collision on alpha must not discard main's still-pending record.
    let records: Value = serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    let references: Vec<&str> = records
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["reference"].as_str().unwrap())
        .collect();
    assert_eq!(references, vec!["refs/heads/main", "refs/heads/alpha"]);

    // Repair on alpha clears only alpha's record.
    repair_pending_checkout_sync(&repo_git, &repo).unwrap();
    assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), alpha_to);
    assert!(git_stdout(&repo, &["status", "--porcelain"]).is_empty());
    let records: Value = serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["reference"], "refs/heads/main");

    // Back on main, repair replays main's record and removes the file.
    git(&repo, &["switch", "main"]).unwrap();
    repair_pending_checkout_sync(&repo_git, &repo).unwrap();
    assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), main_to);
    assert_eq!(
        fs::read_to_string(repo.join("app.txt")).unwrap(),
        "integrated on main\n"
    );
    assert!(!marker_path.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn non_collision_sync_failure_surfaces_and_repair_replays_the_missed_sync() {
    let fixture = DetachedIntegrationFixture::new("detached-index-lock", |worktree| {
        commit_file(worktree, "feature.txt", "candidate\n", "candidate");
    });
    // The human's own tooling holds the shared checkout's index lock, so the
    // post-CAS `read-tree` fails without any working-tree collision. That must
    // surface as an error — not masquerade as a dirty-file collision telling
    // the human to commit files that do not collide — while still leaving a
    // durable pending record so the missed sync heals once the lock is gone.
    let index_lock = fixture.repo.join(".git/index.lock");
    fs::write(&index_lock, "").unwrap();

    let error = fixture.integrate().unwrap_err();
    assert!(error.to_string().contains("index.lock"), "{error}");
    let advanced = git_stdout(&fixture.repo, &["rev-parse", "main"]);
    assert_ne!(
        advanced, fixture.base_commit,
        "the ref advance already happened"
    );
    assert!(!fixture.repo.join("feature.txt").exists());
    let records: Value =
        serde_json::from_str(&fs::read_to_string(fixture.repo.join(MARKER)).unwrap()).unwrap();
    let reason = records[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("without a working-tree collision"),
        "{reason}"
    );
    assert!(
        !reason.contains("colliding files are committed"),
        "a lock failure must not instruct the human to fix a collision: {reason}"
    );

    fs::remove_file(&index_lock).unwrap();
    repair_pending_checkout_sync(&fixture.repo_git(), &fixture.repo).unwrap();
    assert!(!fixture.repo.join(MARKER).exists());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("feature.txt")).unwrap(),
        "candidate\n"
    );
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());

    // The retried integration recognizes the already-advanced target and
    // records the merge commit that is actually on the branch.
    let integration = fixture.integrate().unwrap();
    assert_eq!(integration.target_commit, advanced);
    fixture.finish();
}
