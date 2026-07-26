mod cancellation;
mod capacity;
mod execution;
mod governance;
mod promotion;
mod ready_merge;

use super::*;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::FileProcessSupervisor;
use crate::process::subprocess::ManagedProcess;
use crate::process::supervisor::config::{FileGovernanceService, FileSettingsService};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::quality::{FileQualityService, QualitySettingsPatch};
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::process_control::FileProcessControlService;
use crate::tools::product::work_items::{BulkGoalSelection, FileWorkItemService};
use crate::workflow::capacity::AgentCapacityService;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// A daemon that dies between starting a claim and recording its terminal state
// leaves the claim `Running` on disk while its lease is pruned as dead. Nothing
// reconciled that, and admission counts `Running` claims, so the orphan held a
// concurrency slot for good: with a cap of 1 no further goal could ever start,
// and a restart did not help because startup recovery never touched claims.

/// Builds a repository whose recorded base is no longer an ancestor of the
/// candidate, the way a branch tip advancing under an in-flight round leaves
/// it. When `merge_candidate` is set the candidate is already merged into
/// the target branch first, so the work is present in the branch.
fn ready_merge_goal_with_advanced_target(
    label: &str,
    merge_candidate: bool,
) -> (PathBuf, PathBuf, PathBuf, FileWorkItemService, String) {
    let temp_root = unique_temp_dir(label);
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
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
    fs::write(worktree_path.join("feature.txt"), "candidate work\n").unwrap();
    git(&worktree_path, &["add", "feature.txt"]).unwrap();
    git(&worktree_path, &["commit", "-q", "-m", "candidate"]).unwrap();
    let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    if merge_candidate {
        git(
            &target_root,
            &["merge", "-q", "--no-ff", "-m", "merge candidate", branch],
        )
        .unwrap();
    }
    // The tip then moves on, the way a git-sync merge moves it. The base the
    // merger records is this tip, which is not an ancestor of the candidate.
    fs::write(temp_root.join("unrelated.txt"), "tip moved\n").unwrap();
    git(&target_root, &["add", "unrelated.txt"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "advance the tip"]).unwrap();
    let recorded_base = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    assert_ne!(recorded_base, candidate_commit);

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Advanced target", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .update_latest_goal_round_implementation_report("GOAL1", "Implementation completed")
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
            &recorded_base,
            Some(&candidate_commit),
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

    (
        temp_root,
        target_root,
        worktree_path,
        work_items,
        candidate_commit,
    )
}

fn test_refine_dir(target_root: &Path) -> PathBuf {
    fs::create_dir_all(target_root).unwrap();
    if !target_root.join(".git").exists() {
        git(target_root, &["init", "-q"]).unwrap();
    }
    crate::tools::host::project_layout::refine_dir_for_target_root(target_root).unwrap()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(RefineError::Io(format!(
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}

fn git_stdout(repo: &Path, args: &[&str]) -> RefineResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    Err(RefineError::Io(format!(
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}
