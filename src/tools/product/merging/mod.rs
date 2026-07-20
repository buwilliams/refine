use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::JsonObject;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::product::project_state::GoalSummaryProjection;
use crate::tools::product::work_items::FileWorkItemService;

#[derive(Clone, Debug)]
pub struct FileMergerService {
    pub runtime_root: PathBuf,
    pub refine_dir: PathBuf,
}

impl FileMergerService {
    pub fn new(runtime_root: impl Into<PathBuf>, refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: refine_dir.into(),
        }
    }

    /// Approve a reviewed Goal by integrating its isolated candidate exactly
    /// once. Surfaces expose approval; this capability owns the Git mechanics.
    pub fn approve_reviewed_goal(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            self.runtime_root.join("cache"),
        );
        let goal = work_items.show_goal_summary(goal_id)?;
        if goal.goal.status != GoalStatus::Review {
            return Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} can only be approved from review"
            )));
        }
        let settings =
            FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()?;
        let branch_name = goal
            .goal
            .branch_name
            .as_deref()
            .filter(|branch| !branch.trim().is_empty())
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} does not have an implementation candidate"
                ))
            })?
            .to_string();
        let target_branch = setting_string(&settings, "merge_target_branch", "main");
        let remote = setting_string(&settings, "git_remote", "origin");
        let target_root = target_root(&self.refine_dir)?;

        with_repository_git_lock(&target_root, || {
            let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
            if git.remote_exists(&remote)? {
                git.ensure_branch_from_remote(&remote, &branch_name)?;
            }
            git.switch(&target_branch)?;
            let merge = git.merge_no_ff(&branch_name)?;
            if !merge.ok {
                let _ = git.recover();
                return Err(RefineError::Conflict(merge.message.unwrap_or_else(|| {
                    "implementation integration failed".to_string()
                })));
            }
            git.cleanup_merged_branch(&branch_name)?;
            Ok(())
        })?;

        work_items.verify_goal_summary(goal_id)
    }
}

pub fn branch_name_for_goal(settings: &JsonObject, goal_id: &str) -> String {
    setting_string(settings, "branch_name_pattern", "refine/{goal_id}")
        .replace("{goal_id}", goal_id)
}

pub fn target_root(refine_dir: &Path) -> RefineResult<PathBuf> {
    refine_dir.parent().map(Path::to_path_buf).ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "refine dir {} has no target root",
            refine_dir.display()
        ))
    })
}

fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::tools::product::work_items::FileWorkItemService;

    #[test]
    fn reviewed_goal_approval_integrates_and_cleans_candidate() {
        let temp_root = unique_temp_dir("review-approval-cleanup");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("repo-refine-GOAL1-round-1");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let branch = "refine/GOAL1/round-1";
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        commit_file(&worktree_path, "feature.txt", "change\n", "GOAL1");

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("GOAL1", Some("GOAL1"))
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Build)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Qa)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Review)
            .unwrap();
        work_items.set_goal_branch_name("GOAL1", branch).unwrap();

        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let approved = merger.approve_reviewed_goal("GOAL1").unwrap();
        assert_eq!(approved.goal.status, GoalStatus::Done);
        assert!(!worktree_path.exists());
        assert!(!git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains(branch));
        assert!(!git_succeeds(
            &repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
        ));
        assert_eq!(
            fs::read_to_string(repo.join("feature.txt")).unwrap(),
            "change\n"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn init_repo(repo: &Path) {
        git(repo, &["init", "-b", "main"]).unwrap();
        git(repo, &["config", "user.email", "test@example.com"]).unwrap();
        git(repo, &["config", "user.name", "Test User"]).unwrap();
    }

    fn commit_file(repo: &Path, path: &str, contents: &str, message: &str) {
        fs::write(repo.join(path), contents).unwrap();
        git(repo, &["add", path]).unwrap();
        git(repo, &["commit", "-m", message]).unwrap();
    }

    fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Conflict(
                format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )
                .trim()
                .to_string(),
            ))
        }
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
