use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::*;
use crate::process::subprocess::{ManagedProcess, ProcessOwner};
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::workflow::{
    WORKFLOW_AUTOMATION_STATE_FILE, WorkflowAutomationState, WorkflowClaim, WorkflowClaimState,
    WorkflowPolicy,
};

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
    fixture.create_goal("MERGED", "refine/MERGED/round-1", false);
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
    assert_eq!(
        report.entries[0].branch_cleanup_reason.as_deref(),
        Some("integrated_branch_retired")
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
fn cleanup_discovers_and_retires_an_integrated_branch_after_its_worktree_is_gone() {
    let fixture = Fixture::new("orphaned-integrated-ref");
    fixture.create_goal("ORPHANED", "refine/ORPHANED/round-1", false);
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
    let target = git_output(&fixture.repo, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    FileWorkItemService::new(&fixture.refine_dir)
        .update_goal_round_evaluation_summary(
            "ORPHANED",
            0,
            &json!({
                "workflow_integration": {
                    "candidate_commit": candidate,
                    "target_branch": "main",
                    "target_commit": target,
                    "remote": "origin",
                    "pushed": false,
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
    git(
        &fixture.repo,
        &["worktree", "remove", worktree.to_str().unwrap()],
    );
    let service = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);

    let preview = service.run(WorktreeCleanupOptions::default()).unwrap();
    assert_eq!(preview.inspected, 0);
    assert_eq!(preview.branch_inspected, 1);
    assert_eq!(preview.branch_eligible, 1);
    assert_eq!(
        preview.branch_entries[0].reason,
        "integrated_branch_eligible"
    );

    let report = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(report.removed, 0);
    assert_eq!(report.branches_deleted, 1);
    assert_eq!(report.local_branches_deleted, 1);
    assert_eq!(report.remote_branches_deleted, 0);
    assert!(!git_succeeds(
        &fixture.repo,
        &[
            "rev-parse",
            "--verify",
            "refs/heads/refine/ORPHANED/round-1"
        ]
    ));
}

#[test]
fn cleanup_keeps_an_integrated_branch_that_advanced_beyond_recorded_candidate() {
    let fixture = Fixture::new("advanced-integrated-ref");
    fixture.create_goal("ADVANCED", "refine/ADVANCED/round-1", false);
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
    let target = git_output(&fixture.repo, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    FileWorkItemService::new(&fixture.refine_dir)
        .update_goal_round_evaluation_summary(
            "ADVANCED",
            0,
            &json!({
                "workflow_integration": {
                    "candidate_commit": candidate,
                    "target_branch": "main",
                    "target_commit": target,
                    "remote": "origin",
                    "pushed": false,
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
    fs::write(worktree.join("later.txt"), "later work\n").unwrap();
    git(&worktree, &["add", "later.txt"]);
    git(&worktree, &["commit", "-m", "later candidate work"]);
    let advanced = git_output(&worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 1);
    assert_eq!(report.branches_deleted, 0);
    assert_eq!(
        report.entries[0].branch_cleanup_reason.as_deref(),
        Some("candidate_not_integrated")
    );
    assert_eq!(
        git_output(&fixture.repo, &["rev-parse", "refine/ADVANCED/round-1"]).trim(),
        advanced
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
        .register_with_request("quality", json!({"nested": {"goal_id": "OPERATION"}}))
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
    assert!(reasons.contains(&(Some("OPERATION"), "active_owner")));
    assert!(reasons.contains(&(None, "goal_not_found")));
    assert!(!review.exists(), "hibernated {}", review.display());
    for path in [dirty, active, operation, missing] {
        assert!(path.exists(), "preserved {}", path.display());
    }
}

#[test]
fn cleanup_preserves_a_worktree_owned_by_an_active_workflow_claim() {
    let fixture = Fixture::new("active-claim");
    fixture.create_goal("CLAIMED", "refine/CLAIMED/round-1", false);
    let worktree = fixture.add_worktree("refine/CLAIMED/round-1");
    let state = WorkflowAutomationState {
        version: 1,
        policy: WorkflowPolicy::default(),
        claims: vec![WorkflowClaim {
            claim_id: "claim-1".to_string(),
            goal_id: "CLAIMED".to_string(),
            node_id: "default".to_string(),
            provider: "claude".to_string(),
            target_app_id: "default".to_string(),
            execution_id: None,
            round_idx: Some(0),
            goal_revision: None,
            decision_version: 1,
            state: WorkflowClaimState::Claimed,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }],
        updated_at: Some("2026-01-01T00:00:00Z".to_string()),
    };
    fs::create_dir_all(&fixture.runtime_root).unwrap();
    fs::write(
        fixture.runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 0);
    assert_eq!(report.entries[0].reason, "active_owner");
    assert!(worktree.exists());
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
fn cleanup_preserves_inactive_worktree_with_unrecognized_ignored_content() {
    let fixture = Fixture::new("ignored-user-content");
    fixture.commit_files(&[(".gitignore", ".env\n")]);
    fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
    let worktree = fixture.add_worktree("refine/GOAL1/round-1");
    fs::write(worktree.join(".env"), "SECRET=preserve-me\n").unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.removed, 0);
    assert_eq!(report.entries[0].reason, "ignored_worktree");
    assert!(worktree.join(".env").is_file());
}

#[test]
fn cleanup_removes_detected_generated_cache_before_inactive_worktree() {
    let fixture = Fixture::new("generated-cache");
    fixture.commit_files(&[
        (".gitignore", "/target/\n"),
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        ),
    ]);
    fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
    let worktree = fixture.add_worktree("refine/GOAL1/round-1");
    fs::create_dir_all(worktree.join("target/debug")).unwrap();
    fs::write(worktree.join("target/debug/cache"), "generated\n").unwrap();

    let service = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);
    let preview = service.run(WorktreeCleanupOptions::default()).unwrap();
    assert_eq!(preview.eligible, 1);
    assert_eq!(preview.entries[0].generated_paths, vec!["target"]);
    assert!(worktree.exists());

    let report = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(report.entries[0].generated_paths_removed, 1);
    assert!(!worktree.exists());
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"]
    ));
}

#[test]
fn cleanup_removes_only_ignored_descendants_of_a_configured_generated_root() {
    let fixture = Fixture::new("configured-generated-descendant");
    fixture.commit_files(&[
        (".gitignore", "/build/cache/\n"),
        ("build/keep.txt", "tracked\n"),
    ]);
    FileSettingsService::with_active_root(&fixture.refine_dir, &fixture.runtime_root)
        .update(&json!({"worktree_cleanup_generated_paths": "build"}))
        .unwrap();
    fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
    let worktree = fixture.add_worktree("refine/GOAL1/round-1");
    fs::create_dir_all(worktree.join("build/cache")).unwrap();
    fs::write(worktree.join("build/cache/generated"), "generated\n").unwrap();

    let service = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);
    let preview = service.run(WorktreeCleanupOptions::default()).unwrap();
    assert_eq!(preview.entries[0].generated_paths, vec!["build/cache"]);
    assert_eq!(
        fs::read_to_string(worktree.join("build/keep.txt")).unwrap(),
        "tracked\n"
    );

    let report = service
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();
    assert_eq!(report.removed, 1);
    assert_eq!(report.entries[0].generated_paths_removed, 1);
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
