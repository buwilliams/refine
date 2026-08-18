use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::*;
use crate::infrastructure::process::subprocess::{ManagedProcess, ProcessOwner};
use crate::infrastructure::storage::project_layout::refine_dir_for_target_root;
use crate::model::workflow::GoalStatus;

#[test]
fn cleanup_hibernates_all_clean_inactive_round_worktrees_and_preserves_branches() {
    let fixture = Fixture::new("inactive-rounds");
    fixture.create_goal("GOAL1", "refine/GOAL1/round-2", true);
    let first = fixture.add_worktree("refine/GOAL1/round-1");
    let second = fixture.add_worktree("refine/GOAL1/round-2");

    let service = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);
    let preview = service.run(WorktreeCleanupOptions::default()).unwrap();
    assert_eq!(preview.inspected, 2);
    assert_eq!(preview.eligible, 2);
    assert_eq!(preview.removed, 0);
    assert!(first.exists());
    assert!(second.exists());

    let applied = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(applied.removed, 2);
    assert_eq!(applied.failed, 0);
    assert_eq!(applied.branches_deleted, 0);
    assert!(!first.exists());
    assert!(!second.exists());
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"]
    ));
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-2"]
    ));
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            first.to_str().unwrap(),
            "refine/GOAL1/round-1",
        ],
    );
    assert!(first.exists(), "preserved branch can restore the checkout");
}

#[test]
fn cleanup_retires_exact_integrated_candidate_refs_locally_and_upstream() {
    let fixture = Fixture::new("integrated-refs");
    let remote = fixture.root.join("origin.git");
    git(&fixture.root, &["init", "--bare", remote.to_str().unwrap()]);
    git(
        &fixture.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    fixture.create_goal("MERGED", "refine/MERGED/round-1", true);
    let worktree = fixture.add_worktree("refine/MERGED/round-1");
    fs::write(worktree.join("merged.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "merged.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    let target = git_output(&fixture.repo, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&fixture.repo, &["push", "origin", "refine/MERGED/round-1"]);
    FileWorkItemService::new(&fixture.refine_dir)
        .update_goal_round_evaluation_summary(
            "MERGED",
            0,
            &json!({
                "workflow_integration": {
                    "candidate_commit": candidate,
                    "target_branch": "main",
                    "target_commit": target,
                    "remote": "origin",
                    "pushed": true,
                    "integrated_at": "2026-01-01T00:00:00Z",
                    "merge": {
                        "ok": true,
                        "conflicts": [],
                        "message": "merged"
                    }
                }
            }),
        )
        .unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 1);
    assert_eq!(report.branches_deleted, 1);
    assert_eq!(report.local_branches_deleted, 1);
    assert_eq!(report.remote_branches_deleted, 1);
    assert_eq!(report.branch_entries[0].reason, "retired");
    assert_eq!(
        report.branch_entries[0].local_reason.as_deref(),
        Some("retired")
    );
    assert_eq!(
        report.branch_entries[0].remote_reason.as_deref(),
        Some("retired")
    );
    assert!(!worktree.exists());
    assert!(!git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/MERGED/round-1"]
    ));
    assert!(
        git_output(
            &fixture.repo,
            &["ls-remote", "--heads", "origin", "refine/MERGED/round-1"]
        )
        .trim()
        .is_empty()
    );
    assert!(git_succeeds(
        &fixture.repo,
        &["merge-base", "--is-ancestor", &candidate, "main"]
    ));
}

#[test]
fn cleanup_discovers_and_retires_a_remote_only_branch_for_a_deleted_goal() {
    let fixture = Fixture::new("remote-only-deleted-goal-ref");
    fixture.add_origin();
    let worktree = fixture.add_worktree("refine/ORPHANED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(
        &fixture.repo,
        &["push", "origin", "refine/ORPHANED/round-1"],
    );
    git(
        &fixture.repo,
        &["worktree", "remove", worktree.to_str().unwrap()],
    );
    git(&fixture.repo, &["branch", "-D", "refine/ORPHANED/round-1"]);
    let service = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);

    let retained = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 3600,
        })
        .unwrap();
    assert_eq!(retained.remote_branches_deleted, 0);
    assert_eq!(retained.branch_entries[0].reason, "retention_window");

    let preview = service.run(WorktreeCleanupOptions::default()).unwrap();
    assert_eq!(preview.inspected, 0);
    assert_eq!(preview.branch_inspected, 1);
    assert_eq!(preview.branch_eligible, 1);
    assert_eq!(preview.branch_entries[0].reason, "remote_eligible");
    assert_eq!(
        preview.branch_entries[0].goal_id.as_deref(),
        Some("ORPHANED")
    );
    assert!(!preview.branch_entries[0].local_present);
    assert!(preview.branch_entries[0].remote_present);

    let report = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(report.removed, 0);
    assert_eq!(report.branches_deleted, 1);
    assert_eq!(report.local_branches_deleted, 0);
    assert_eq!(report.remote_branches_deleted, 1);
    assert!(
        git_output(
            &fixture.repo,
            &["ls-remote", "--heads", "origin", "refine/ORPHANED/round-1"]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn cleanup_preserves_a_missing_goal_ref_with_active_goal_ownership() {
    let fixture = Fixture::new("active-missing-goal-ref");
    fixture.add_origin();
    let worktree = fixture.add_worktree("refine/OWNED_MISSING/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(
        &worktree,
        &["push", "origin", "refine/OWNED_MISSING/round-1"],
    );
    git(
        &fixture.repo,
        &["worktree", "remove", worktree.to_str().unwrap()],
    );
    git(
        &fixture.repo,
        &["branch", "-D", "refine/OWNED_MISSING/round-1"],
    );
    FileOperationRegistry::new(&fixture.runtime_root)
        .register_with_request(
            "active-missing-goal",
            json!({
                "kind": "workflow_candidate_handoff",
                "goal_id": "OWNED_MISSING"
            }),
        )
        .unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.branches_deleted, 0);
    assert_eq!(report.branch_entries[0].reason, "active_owner");
    assert!(
        !git_output(
            &fixture.repo,
            &[
                "ls-remote",
                "--heads",
                "origin",
                "refine/OWNED_MISSING/round-1"
            ]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn cleanup_keeps_terminal_branch_work_that_is_not_in_the_remote_target() {
    let fixture = Fixture::new("unmerged-terminal-ref");
    fixture.add_origin();
    fixture.create_goal("ADVANCED", "refine/ADVANCED/round-1", true);
    let worktree = fixture.add_worktree("refine/ADVANCED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    fs::write(worktree.join("later.txt"), "later work\n").unwrap();
    git(&worktree, &["add", "later.txt"]);
    git(&worktree, &["commit", "-m", "later candidate work"]);
    let advanced = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(&worktree, &["push", "origin", "refine/ADVANCED/round-1"]);

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 1);
    assert_eq!(report.branches_deleted, 0);
    assert_eq!(report.branch_entries[0].reason, "not_in_remote_target");
    assert_eq!(
        report.branch_entries[0].remote_reason.as_deref(),
        Some("not_in_remote_target")
    );
    assert_eq!(
        git_output(&fixture.repo, &["rev-parse", "refine/ADVANCED/round-1"]).trim(),
        advanced
    );
}

#[test]
fn cleanup_retires_only_the_remote_side_of_a_checked_out_terminal_branch() {
    let fixture = Fixture::new("checked-out-terminal-ref");
    fixture.add_origin();
    fixture.create_goal("CHECKED", "refine/CHECKED/round-1", true);
    let worktree = fixture.add_worktree("refine/CHECKED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&worktree, &["push", "origin", "refine/CHECKED/round-1"]);
    fs::write(worktree.join("untracked.txt"), "keep checkout\n").unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 0);
    assert_eq!(report.local_branches_deleted, 0);
    assert_eq!(report.remote_branches_deleted, 1);
    assert!(worktree.exists());
    let branch = &report.branch_entries[0];
    assert_eq!(branch.local_reason.as_deref(), Some("checked_out"));
    assert_eq!(branch.remote_reason.as_deref(), Some("retired"));
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/CHECKED/round-1"]
    ));
}

#[test]
fn cleanup_preserves_both_sides_while_a_worktree_path_has_active_ownership() {
    let fixture = Fixture::new("actively-owned-terminal-ref");
    fixture.add_origin();
    fixture.create_goal("OWNED", "refine/OWNED/round-1", true);
    let worktree = fixture.add_worktree("refine/OWNED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&worktree, &["push", "origin", "refine/OWNED/round-1"]);
    fixture.register_active_process(&worktree);

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.branches_deleted, 0);
    assert_eq!(report.branch_entries[0].reason, "active_owner");
    assert!(worktree.exists());
    assert!(
        !git_output(
            &fixture.repo,
            &["ls-remote", "--heads", "origin", "refine/OWNED/round-1"]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn cleanup_uses_the_configured_remote_and_merge_target_for_done_goals() {
    let fixture = Fixture::new("configured-retirement-authority");
    fixture.add_origin();
    git(&fixture.repo, &["remote", "rename", "origin", "upstream"]);
    fixture.create_goal("CONFIGURED", "refine/CONFIGURED/round-1", false);
    FileWorkItemService::new(&fixture.refine_dir)
        .set_goal_status_unchecked("CONFIGURED", &GoalStatus::Done)
        .unwrap();
    FileSettingsService::with_active_root(&fixture.refine_dir, &fixture.runtime_root)
        .update(&json!({
            "git_remote": "upstream",
            "merge_target_branch": "release"
        }))
        .unwrap();
    let worktree = fixture.add_worktree("refine/CONFIGURED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(&fixture.repo, &["branch", "release", &candidate]);
    git(&fixture.repo, &["push", "upstream", "release"]);
    git(
        &worktree,
        &["push", "upstream", "refine/CONFIGURED/round-1"],
    );
    fs::write(worktree.join("untracked.txt"), "keep checkout\n").unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.remote_branches_deleted, 1);
    assert_eq!(report.local_branches_deleted, 0);
    assert_eq!(
        report.branch_entries[0].goal_status.as_deref(),
        Some("done")
    );
    assert_eq!(
        report.branch_entries[0].remote_reason.as_deref(),
        Some("retired")
    );
}

#[test]
fn cleanup_retires_ancestry_proven_superseded_round_refs() {
    let fixture = Fixture::new("superseded-round-refs");
    fixture.add_origin();
    fixture.create_goal("SUPERSEDED", "refine/SUPERSEDED/round-2", true);
    let first = fixture.add_worktree("refine/SUPERSEDED/round-1");
    fs::write(first.join("first.txt"), "first\n").unwrap();
    git(&first, &["add", "first.txt"]);
    git(&first, &["commit", "-m", "first round"]);
    let first_commit = git_output(&first, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &first_commit],
    );
    let second = fixture.add_worktree("refine/SUPERSEDED/round-2");
    fs::write(second.join("second.txt"), "second\n").unwrap();
    git(&second, &["add", "second.txt"]);
    git(&second, &["commit", "-m", "second round"]);
    let second_commit = git_output(&second, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &second_commit],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&first, &["push", "origin", "refine/SUPERSEDED/round-1"]);
    git(&second, &["push", "origin", "refine/SUPERSEDED/round-2"]);

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 2);
    assert_eq!(report.branch_inspected, 2);
    assert_eq!(report.local_branches_deleted, 2);
    assert_eq!(report.remote_branches_deleted, 2);
    assert!(
        report
            .branch_entries
            .iter()
            .all(|entry| entry.reason == "retired")
    );
}

#[test]
fn cleanup_preserves_active_state_and_malformed_remote_refs() {
    let fixture = Fixture::new("protected-remote-refs");
    fixture.add_origin();
    fixture.create_goal("ACTIVE", "refine/ACTIVE/round-1", false);
    fixture.create_goal("FAILED", "refine/FAILED/round-1", false);
    fixture.create_goal("OTHER", "refine/AMBIGUOUS/round-1", true);
    FileWorkItemService::new(&fixture.refine_dir)
        .set_goal_status_unchecked("FAILED", &GoalStatus::Failed)
        .unwrap();
    let worktree = fixture.add_worktree("refine/ACTIVE/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&worktree, &["push", "origin", "refine/ACTIVE/round-1"]);
    git(&fixture.repo, &["branch", "refine/FAILED/round-1"]);
    git(&fixture.repo, &["push", "origin", "refine/FAILED/round-1"]);
    git(&fixture.repo, &["branch", "refine/AMBIGUOUS/round-1"]);
    git(
        &fixture.repo,
        &["push", "origin", "refine/AMBIGUOUS/round-1"],
    );
    git(&fixture.repo, &["push", "origin", "main:refine/state"]);
    git(&fixture.repo, &["push", "origin", "main:refine/MALFORMED"]);

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.remote_branches_deleted, 0);
    let reasons = report
        .branch_entries
        .iter()
        .map(|entry| (entry.branch.as_str(), entry.reason.as_str()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(reasons["refine/ACTIVE/round-1"], "goal_not_terminal");
    assert_eq!(reasons["refine/FAILED/round-1"], "goal_not_terminal");
    assert_eq!(reasons["refine/AMBIGUOUS/round-1"], "ambiguous_goal");
    assert_eq!(reasons["refine/state"], "protected_state_branch");
    assert_eq!(reasons["refine/MALFORMED"], "malformed_branch");
}

#[test]
fn atomic_remote_delete_preserves_a_branch_when_the_target_snapshot_moves() {
    let fixture = Fixture::new("target-moved-fence");
    fixture.add_origin();
    let worktree = fixture.add_worktree("refine/FENCED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&worktree, &["push", "origin", "refine/FENCED/round-1"]);
    let old_target = git_output(&fixture.repo, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    fs::write(fixture.repo.join("later.txt"), "target moved\n").unwrap();
    git(&fixture.repo, &["add", "later.txt"]);
    git(&fixture.repo, &["commit", "-m", "advance target"]);
    git(&fixture.repo, &["push", "origin", "main"]);

    let outcome = FileGitWorktreeService::new(&fixture.repo)
        .delete_remote_branch_if_snapshot_matches(
            "origin",
            "refine/FENCED/round-1",
            &candidate,
            "main",
            &old_target,
        )
        .unwrap();

    assert_eq!(
        outcome,
        crate::infrastructure::git::worktrees::GitRemoteRefDeleteOutcome::TargetChanged
    );
    assert!(
        !git_output(
            &fixture.repo,
            &["ls-remote", "--heads", "origin", "refine/FENCED/round-1"]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn atomic_remote_delete_preserves_a_branch_when_the_candidate_moves() {
    let fixture = Fixture::new("candidate-moved-fence");
    fixture.add_origin();
    let worktree = fixture.add_worktree("refine/FENCED/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&worktree, &["push", "origin", "refine/FENCED/round-1"]);
    let target = git_output(&fixture.repo, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    fs::write(worktree.join("later.txt"), "candidate moved\n").unwrap();
    git(&worktree, &["add", "later.txt"]);
    git(&worktree, &["commit", "-m", "advance candidate"]);
    git(&worktree, &["push", "origin", "refine/FENCED/round-1"]);

    let outcome = FileGitWorktreeService::new(&fixture.repo)
        .delete_remote_branch_if_snapshot_matches(
            "origin",
            "refine/FENCED/round-1",
            &candidate,
            "main",
            &target,
        )
        .unwrap();

    assert_eq!(
        outcome,
        crate::infrastructure::git::worktrees::GitRemoteRefDeleteOutcome::BranchChanged
    );
    assert!(
        !git_output(
            &fixture.repo,
            &["ls-remote", "--heads", "origin", "refine/FENCED/round-1"]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn cleanup_preserves_both_refs_when_the_remote_rejects_atomic_push() {
    let fixture = Fixture::new("atomic-push-unsupported");
    fixture.add_origin();
    git(
        &fixture.root.join("origin.git"),
        &["config", "receive.advertiseAtomic", "false"],
    );
    fixture.create_goal("ATOMIC", "refine/ATOMIC/round-1", true);
    let worktree = fixture.add_worktree("refine/ATOMIC/round-1");
    fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "candidate.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.repo,
        &["merge", "--no-ff", "--no-edit", &candidate],
    );
    git(&fixture.repo, &["push", "origin", "main"]);
    git(&worktree, &["push", "origin", "refine/ATOMIC/round-1"]);

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.branches_deleted, 0);
    assert_eq!(report.branch_entries[0].reason, "atomic_delete_unsupported");
    assert_eq!(
        report.branch_entries[0].local_reason.as_deref(),
        Some("atomic_delete_unsupported")
    );
    assert_eq!(
        report.branch_entries[0].remote_reason.as_deref(),
        Some("atomic_delete_unsupported")
    );
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/ATOMIC/round-1"]
    ));
    assert!(
        !git_output(
            &fixture.repo,
            &["ls-remote", "--heads", "origin", "refine/ATOMIC/round-1"]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn cleanup_preserves_dirty_missing_and_owned_worktrees_but_hibernates_inactive_statuses() {
    let fixture = Fixture::new("safety");
    fixture.create_goal("DIRTY", "refine/DIRTY/round-1", true);
    fixture.create_goal("ACTIVE", "refine/ACTIVE/round-1", true);
    fixture.create_goal("REVIEW", "refine/REVIEW/round-1", false);
    fixture.create_goal("OPERATION", "refine/OPERATION/round-1", false);
    let dirty = fixture.add_worktree("refine/DIRTY/round-1");
    let active = fixture.add_worktree("refine/ACTIVE/round-1");
    let review = fixture.add_worktree("refine/REVIEW/round-1");
    let operation = fixture.add_worktree("refine/OPERATION/round-1");
    let missing = fixture.add_worktree("refine/MISSING/round-1");
    fs::write(dirty.join("untracked.txt"), "preserve me\n").unwrap();
    fixture.register_active_process(&active);
    FileOperationRegistry::new(&fixture.runtime_root)
        .register_with_request(
            "candidate-handoff",
            json!({
                "kind": "workflow_candidate_handoff",
                "nested": {"worktree_path": operation}
            }),
        )
        .unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 1);
    let reasons = report
        .entries
        .iter()
        .map(|entry| (entry.goal_id.as_deref(), entry.reason.as_str()))
        .collect::<Vec<_>>();
    assert!(reasons.contains(&(Some("DIRTY"), "dirty_worktree")));
    assert!(reasons.contains(&(Some("ACTIVE"), "active_process")));
    assert!(reasons.contains(&(Some("REVIEW"), "eligible")));
    assert!(reasons.contains(&(Some("OPERATION"), "active_process")));
    assert!(reasons.contains(&(None, "goal_not_found")));
    assert!(!review.exists(), "hibernated {}", review.display());
    for path in [dirty, active, operation, missing] {
        assert!(path.exists(), "preserved {}", path.display());
    }
}

#[test]
fn cleanup_preserves_automated_status_worktrees_without_live_runtime_ownership() {
    let fixture = Fixture::new("automated-status-restart-gap");
    let mut worktrees = Vec::new();
    for (goal_id, status) in [
        ("PLAN", GoalStatus::Plan),
        ("IMPLEMENT", GoalStatus::Implement),
        ("QUALITY", GoalStatus::Quality),
        ("GOVERNANCE", GoalStatus::Governance),
    ] {
        let branch = format!("refine/{goal_id}/round-1");
        fixture.create_goal(goal_id, &branch, false);
        FileWorkItemService::new(&fixture.refine_dir)
            .set_goal_status_unchecked(goal_id, &status)
            .unwrap();
        worktrees.push(fixture.add_worktree(&branch));
    }

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 0);
    assert_eq!(report.preserved, 4);
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.reason == "active_goal_status")
    );
    for worktree in worktrees {
        assert!(worktree.exists(), "preserved {}", worktree.display());
    }
}

#[test]
fn cleanup_retention_window_and_disable_setting_fail_closed() {
    let fixture = Fixture::new("retention");
    fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
    let worktree = fixture.add_worktree("refine/GOAL1/round-1");
    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 3600,
        })
        .unwrap();
    assert_eq!(report.removed, 0);
    assert_eq!(report.entries[0].reason, "retention_window");
    assert!(worktree.exists());

    let mut settings = serde_json::Map::new();
    assert_eq!(automatic_cleanup_delay_seconds(&settings), Some(0));
    settings.insert("worktree_cleanup_after_seconds".to_string(), json!("-1"));
    assert_eq!(automatic_cleanup_delay_seconds(&settings), None);
    settings.insert("worktree_cleanup_after_seconds".to_string(), json!("3600"));
    assert_eq!(automatic_cleanup_delay_seconds(&settings), Some(3600));
}

#[test]
fn cleanup_discards_ignored_content_when_hibernating_an_inactive_checkout() {
    let fixture = Fixture::new("ignored-content");
    fixture.commit_files(&[
        (".gitignore", ".env\n/target/\n"),
        ("build/keep.txt", "tracked\n"),
    ]);
    fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
    let worktree = fixture.add_worktree("refine/GOAL1/round-1");
    fs::write(worktree.join(".env"), "RUNTIME=disposable\n").unwrap();
    fs::create_dir_all(worktree.join("target/debug")).unwrap();
    fs::write(worktree.join("target/debug/cache"), "generated\n").unwrap();

    let service = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);
    let preview = service.run(WorktreeCleanupOptions::default()).unwrap();
    assert_eq!(preview.eligible, 1);
    assert_eq!(preview.entries[0].reason, "eligible");
    assert!(worktree.exists());

    let report = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(report.removed, 1);
    assert!(!worktree.exists());
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"]
    ));
    assert_eq!(
        git_output(
            &fixture.repo,
            &["show", "refine/GOAL1/round-1:build/keep.txt"]
        ),
        "tracked\n"
    );
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    runtime_root: PathBuf,
    refine_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = unique_temp_dir(name);
        let repo = root.join("repo");
        let runtime_root = root.join("runtime");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test User"]);
        fs::write(repo.join("README.md"), "base\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "base"]);
        let refine_dir = refine_dir_for_target_root(&repo).unwrap();
        fs::create_dir_all(&refine_dir).unwrap();
        Self {
            root,
            repo,
            runtime_root,
            refine_dir,
        }
    }

    fn add_origin(&self) {
        let remote = self.root.join("origin.git");
        git(&self.root, &["init", "--bare", remote.to_str().unwrap()]);
        git(
            &self.repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
    }

    fn create_goal(&self, id: &str, branch: &str, terminal: bool) {
        let work_items = FileWorkItemService::new(&self.refine_dir);
        work_items
            .create_goal_summary(&format!("{id} work"), Some(id))
            .unwrap();
        work_items
            .append_goal_round_summary(id, "Tester", "Implement")
            .unwrap();
        work_items.set_goal_branch_name(id, branch).unwrap();
        if terminal {
            work_items.cancel_goal_summary(id).unwrap();
        }
    }

    fn commit_files(&self, files: &[(&str, &str)]) {
        for (relative, contents) in files {
            let path = self.repo.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, contents).unwrap();
            git(&self.repo, &["add", relative]);
        }
        git(&self.repo, &["commit", "-m", "fixture files"]);
    }

    fn add_worktree(&self, branch: &str) -> PathBuf {
        let path = self
            .repo
            .join(".git/refine-worktrees")
            .join(branch.replace('/', "-"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        git(
            &self.repo,
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
        path
    }

    fn register_active_process(&self, worktree: &Path) {
        let processes = self.runtime_root.join("processes");
        fs::create_dir_all(&processes).unwrap();
        let process = ManagedProcess {
            id: "active-agent".to_string(),
            owner: ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("Active Goal Agent".to_string()),
            details: Some(
                json!({
                    "kind": "workflow",
                    "goal_id": "ACTIVE",
                    "worktree": {"path": worktree}
                })
                .to_string(),
            ),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            exit_code: None,
        };
        fs::write(
            processes.join("active-agent.json"),
            serde_json::to_vec_pretty(&process).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_succeeds(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap()
        .status
        .success()
}

fn git_output(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-worktree-cleanup-{prefix}-{}-{nanos}",
        std::process::id()
    ))
}
