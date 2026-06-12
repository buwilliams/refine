use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::model::feature::compare_feature_gap_order;
use crate::model::log::LogEntry;
use crate::model::workflow::GapStatus;
use crate::process::subprocess::FileProcessSupervisor;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::git_worktrees::{
    FileGitWorktreeService, GitWorktreeService, MergeResult, MergedBranchCleanup,
};
use crate::tools::product::project_state::{FileProjectStateStore, GapSummaryProjection};
use crate::tools::product::work_items::FileWorkItemService;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergerTickResult {
    pub processed: Option<MergerGapResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergerGapResult {
    pub gap_id: String,
    pub branch_name: String,
    pub target_branch: String,
    pub operation_id: String,
    pub status: String,
    pub conflicts: Vec<String>,
    pub cleanup: Option<MergedBranchCleanup>,
}

#[derive(Clone, Debug)]
pub struct FileMergerService {
    pub runtime_root: PathBuf,
    pub refine_dir: PathBuf,
    pub operation_registry: FileOperationRegistry,
}

impl FileMergerService {
    pub fn new(runtime_root: impl Into<PathBuf>, refine_dir: impl Into<PathBuf>) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            operation_registry: FileOperationRegistry::new(&runtime_root),
            runtime_root,
            refine_dir: refine_dir.into(),
        }
    }

    pub fn tick(&self) -> RefineResult<MergerTickResult> {
        if FileProcessSupervisor::new(&self.runtime_root)
            .pause_state()?
            .background_processes_stopped
        {
            return Ok(MergerTickResult { processed: None });
        }
        if self.active_merger_operation()? {
            return Ok(MergerTickResult { processed: None });
        }
        let Some(gap) = self.next_ready_merge_gap()? else {
            return Ok(MergerTickResult { processed: None });
        };
        let gap_id = gap.gap.id.clone();
        let settings = FileSettingsService::new(&self.refine_dir).load()?;
        let branch_name = gap
            .gap
            .branch_name
            .clone()
            .unwrap_or_else(|| branch_name_for_gap(&settings, &gap_id));
        let target_branch = setting_string(&settings, "merge_target_branch", "main");
        let operation = self
            .operation_registry
            .register(&format!("merger:{gap_id}"))?;
        self.append_operation_log(
            &operation.id,
            &gap_id,
            "info",
            "Merging Gap branch",
            Some(json_object(json!({
                "branch_name": branch_name,
                "target_branch": target_branch
            }))),
        )?;
        let result = self.merge_gap_branch(&gap_id, &branch_name, &target_branch, &operation.id);
        match result {
            Ok(merge_result) => Ok(MergerTickResult {
                processed: Some(merge_result),
            }),
            Err(error) => {
                let _ = self.operation_registry.fail_with_error(
                    &operation.id,
                    json!({
                        "gap_id": gap_id,
                        "branch_name": branch_name,
                        "target_branch": target_branch,
                        "error": error.to_string()
                    }),
                );
                Err(error)
            }
        }
    }

    pub fn merge_branch_for_workflow(&self, branch_name: &str) -> RefineResult<MergeResult> {
        let target_root = target_root(&self.refine_dir)?;
        let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
        let mut result = git.merge(branch_name)?;
        for _ in 0..5 {
            if result.ok || !merge_message_has_index_lock(&result) {
                if result.ok {
                    git.cleanup_merged_branch(branch_name)?;
                }
                return Ok(result);
            }
            thread::sleep(Duration::from_millis(50));
            result = git.merge(branch_name)?;
        }
        if result.ok {
            git.cleanup_merged_branch(branch_name)?;
        }
        Ok(result)
    }

    fn merge_gap_branch(
        &self,
        gap_id: &str,
        branch_name: &str,
        target_branch: &str,
        operation_id: &str,
    ) -> RefineResult<MergerGapResult> {
        let target_root = target_root(&self.refine_dir)?;
        let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
        git.switch(target_branch)?;
        let merge = git.merge_no_ff(branch_name)?;
        if !merge.ok {
            let recover = git.recover()?;
            self.fail_gap_merge(
                gap_id,
                operation_id,
                branch_name,
                target_branch,
                &merge,
                &recover,
            )?;
            return Ok(MergerGapResult {
                gap_id: gap_id.to_string(),
                branch_name: branch_name.to_string(),
                target_branch: target_branch.to_string(),
                operation_id: operation_id.to_string(),
                status: "failed".to_string(),
                conflicts: merge.conflicts,
                cleanup: None,
            });
        }
        let cleanup = git.cleanup_merged_branch(branch_name)?;
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            self.runtime_root.join("cache"),
        );
        self.append_operation_log(
            operation_id,
            gap_id,
            "info",
            "Cleaned up merged Gap branch worktree",
            Some(json_object(json!({"cleanup": &cleanup}))),
        )?;
        work_items.advance_automated_gap_status(gap_id, GapStatus::Build)?;
        self.append_operation_log(
            operation_id,
            gap_id,
            "info",
            "Workflow status changed: ready-merge -> build",
            Some(json_object(json!({"merge": &merge}))),
        )?;
        work_items.advance_automated_gap_status(gap_id, GapStatus::Qa)?;
        self.append_operation_log(
            operation_id,
            gap_id,
            "info",
            "Workflow status changed: build -> qa",
            None,
        )?;
        self.operation_registry.finish_with_result(
            operation_id,
            OperationState::Succeeded,
            json!({
                "gap_id": gap_id,
                "branch_name": branch_name,
                "target_branch": target_branch,
                "merge": merge,
                "cleanup": &cleanup,
                "final_status": "qa"
            }),
        )?;
        Ok(MergerGapResult {
            gap_id: gap_id.to_string(),
            branch_name: branch_name.to_string(),
            target_branch: target_branch.to_string(),
            operation_id: operation_id.to_string(),
            status: "qa".to_string(),
            conflicts: Vec::new(),
            cleanup: Some(cleanup),
        })
    }

    fn fail_gap_merge(
        &self,
        gap_id: &str,
        operation_id: &str,
        branch_name: &str,
        target_branch: &str,
        merge: &MergeResult,
        recover: &MergeResult,
    ) -> RefineResult<()> {
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            self.runtime_root.join("cache"),
        );
        work_items.advance_automated_gap_status(gap_id, GapStatus::Failed)?;
        self.append_operation_log(
            operation_id,
            gap_id,
            "error",
            "Gap branch merge failed",
            Some(json_object(json!({
                "branch_name": branch_name,
                "target_branch": target_branch,
                "merge": merge,
                "recover": recover
            }))),
        )?;
        self.operation_registry.fail_with_error(
            operation_id,
            json!({
                "gap_id": gap_id,
                "branch_name": branch_name,
                "target_branch": target_branch,
                "conflicts": merge.conflicts,
                "message": merge.message
            }),
        )?;
        Ok(())
    }

    fn next_ready_merge_gap(&self) -> RefineResult<Option<GapSummaryProjection>> {
        let snapshot =
            FileProjectStateStore::with_runtime_root(&self.refine_dir, &self.runtime_root)
                .load_or_refresh_projection(&self.runtime_root.join("cache"))?;
        let mut candidates = snapshot
            .gaps
            .values()
            .filter(|gap| gap.gap.status == GapStatus::ReadyMerge)
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            compare_feature_gap_order(a.gap.feature_order, b.gap.feature_order)
                .then_with(|| a.gap.updated.cmp(&b.gap.updated))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });
        Ok(candidates.into_iter().next())
    }

    fn active_merger_operation(&self) -> RefineResult<bool> {
        Ok(self
            .operation_registry
            .recover()?
            .into_iter()
            .any(|operation| {
                operation.owner.starts_with("merger:")
                    && matches!(
                        operation.state,
                        OperationState::Pending
                            | OperationState::Running
                            | OperationState::Cancelling
                    )
            }))
    }

    fn append_operation_log(
        &self,
        operation_id: &str,
        gap_id: &str,
        severity: &str,
        message: &str,
        details: Option<JsonObject>,
    ) -> RefineResult<()> {
        self.operation_registry.append_log(
            operation_id,
            LogEntry {
                datetime: now_timestamp(),
                severity: severity.to_string(),
                category: "merger".to_string(),
                message: message.to_string(),
                details,
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                gap_id: Some(gap_id.to_string()),
            },
        )?;
        Ok(())
    }
}

pub fn branch_name_for_gap(settings: &JsonObject, gap_id: &str) -> String {
    setting_string(settings, "branch_name_pattern", "refine/{gap_id}").replace("{gap_id}", gap_id)
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

fn json_object(value: serde_json::Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn merge_message_has_index_lock(result: &MergeResult) -> bool {
    result
        .message
        .as_deref()
        .is_some_and(|message| message.contains("index.lock"))
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
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
    fn file_merger_merges_one_ready_gap_per_tick_with_no_ff_commit() {
        let temp_root = unique_temp_dir("merger-success");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let work_items = FileWorkItemService::new(&refine_dir);
        for id in ["GAP1", "GAP2"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
                .unwrap();
            work_items
                .advance_automated_gap_status(id, GapStatus::InProgress)
                .unwrap();
            work_items
                .advance_automated_gap_status(id, GapStatus::ReadyMerge)
                .unwrap();
            work_items
                .set_gap_branch_name(id, &format!("refine/{id}"))
                .unwrap();
            git(&repo, &["switch", "-c", &format!("refine/{id}")]).unwrap();
            commit_file(&repo, &format!("{id}.txt"), "change\n", id);
            git(&repo, &["switch", "main"]).unwrap();
        }

        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let first = merger.tick().unwrap().processed.unwrap();
        assert_eq!(first.gap_id, "GAP1");
        assert_eq!(first.cleanup.as_ref().unwrap().branch, "refine/GAP1");
        assert!(!first.cleanup.as_ref().unwrap().worktree_removed);
        assert!(first.cleanup.as_ref().unwrap().branch_deleted);
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Qa
        );
        assert_eq!(
            work_items.show_gap_summary("GAP2").unwrap().gap.status,
            GapStatus::ReadyMerge
        );
        assert_eq!(
            git_stdout(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "main"
        );
        assert_eq!(
            git_stdout(&repo, &["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3
        );

        let second = merger.tick().unwrap().processed.unwrap();
        assert_eq!(second.gap_id, "GAP2");
        assert_eq!(
            work_items.show_gap_summary("GAP2").unwrap().gap.status,
            GapStatus::Qa
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_merger_cleans_gap_worktree_and_branch_after_successful_tick() {
        let temp_root = unique_temp_dir("merger-cleanup");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("repo-refine-GAP1-round-1");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let branch = "refine/GAP1/round-1";
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
        commit_file(&worktree_path, "feature.txt", "change\n", "GAP1");

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items.create_gap_summary("GAP1", Some("GAP1")).unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_gap_status("GAP1", GapStatus::InProgress)
            .unwrap();
        work_items
            .advance_automated_gap_status("GAP1", GapStatus::ReadyMerge)
            .unwrap();
        work_items.set_gap_branch_name("GAP1", branch).unwrap();

        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let merged = merger.tick().unwrap().processed.unwrap();
        let cleanup = merged.cleanup.unwrap();
        assert_eq!(merged.gap_id, "GAP1");
        assert_eq!(merged.status, "qa");
        assert_eq!(cleanup.branch, branch);
        assert_eq!(
            cleanup.worktree_path.as_deref(),
            Some(worktree_path.to_str().unwrap())
        );
        assert!(cleanup.worktree_removed);
        assert!(cleanup.branch_deleted);
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

    #[test]
    fn workflow_merge_cleans_gap_worktree_and_branch() {
        let temp_root = unique_temp_dir("workflow-merge-cleanup");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("repo-refine-GAP1-round-1");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let branch = "refine/GAP1/round-1";
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
        commit_file(&worktree_path, "workflow.txt", "change\n", "GAP1");

        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let merge = merger.merge_branch_for_workflow(branch).unwrap();
        assert!(merge.ok);
        assert!(!worktree_path.exists());
        assert!(!git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains(branch));
        assert!(!git_succeeds(
            &repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
        ));
        assert_eq!(
            fs::read_to_string(repo.join("workflow.txt")).unwrap(),
            "change\n"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_merger_marks_only_conflicted_gap_failed_and_recovers_target() {
        let temp_root = unique_temp_dir("merger-conflict");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let work_items = FileWorkItemService::new(&refine_dir);
        for id in ["AAA", "ZZZ"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
                .unwrap();
            work_items
                .advance_automated_gap_status(id, GapStatus::InProgress)
                .unwrap();
            work_items
                .advance_automated_gap_status(id, GapStatus::ReadyMerge)
                .unwrap();
            work_items
                .set_gap_branch_name(id, &format!("refine/{id}"))
                .unwrap();
        }

        git(&repo, &["switch", "-c", "refine/AAA"]).unwrap();
        commit_file(&repo, "app.txt", "branch\n", "branch side");
        git(&repo, &["switch", "main"]).unwrap();
        commit_file(&repo, "app.txt", "main\n", "main side");
        git(&repo, &["switch", "-c", "refine/ZZZ"]).unwrap();
        commit_file(&repo, "clean.txt", "clean\n", "clean side");
        git(&repo, &["switch", "main"]).unwrap();

        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let conflicted = merger.tick().unwrap().processed.unwrap();
        assert_eq!(conflicted.status, "failed");
        assert_eq!(conflicted.conflicts, vec!["app.txt"]);
        assert_eq!(
            work_items.show_gap_summary("AAA").unwrap().gap.status,
            GapStatus::Failed
        );
        assert_eq!(fs::read_to_string(repo.join("app.txt")).unwrap(), "main\n");

        let clean = merger.tick().unwrap().processed.unwrap();
        assert_eq!(clean.gap_id, "ZZZ");
        assert_eq!(
            work_items.show_gap_summary("ZZZ").unwrap().gap.status,
            GapStatus::Qa
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
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
