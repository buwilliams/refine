use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcess};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitLinkedWorktree};
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::work_items::FileWorkItemService;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorktreeCleanupOptions {
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub older_than_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorktreeCleanupEntry {
    pub path: String,
    pub branch: Option<String>,
    pub goal_id: Option<String>,
    pub goal_status: Option<String>,
    pub eligible: bool,
    pub removed: bool,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generated_paths: Vec<String>,
    #[serde(default)]
    pub generated_paths_removed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorktreeCleanupReport {
    pub target_root: String,
    pub apply: bool,
    pub older_than_seconds: u64,
    pub inspected: usize,
    pub eligible: usize,
    pub removed: usize,
    pub preserved: usize,
    pub failed: usize,
    pub branches_deleted: usize,
    pub entries: Vec<WorktreeCleanupEntry>,
}

#[derive(Clone, Debug)]
pub struct FileWorktreeCleanupService {
    target_root: PathBuf,
    runtime_root: PathBuf,
}

impl FileWorktreeCleanupService {
    pub fn new(target_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            target_root: target_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn run(&self, options: WorktreeCleanupOptions) -> RefineResult<WorktreeCleanupReport> {
        let _coordination = acquire_workflow_coordination(&self.runtime_root)?;
        with_repository_git_lock(&self.target_root, || self.run_locked(options))
    }

    fn run_locked(&self, options: WorktreeCleanupOptions) -> RefineResult<WorktreeCleanupReport> {
        let refine_dir = refine_dir_for_target_root(&self.target_root)?;
        let goals = FileWorkItemService::new(&refine_dir)
            .list_goal_summaries()?
            .into_iter()
            .map(|summary| (summary.goal.id.clone(), summary.goal))
            .collect::<BTreeMap<_, _>>();
        let settings =
            FileSettingsService::with_active_root(&refine_dir, &self.runtime_root).load()?;
        let configured_generated_paths = configured_generated_paths(&settings);
        let git = FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root);
        let managed_root = git.git_path("refine-worktrees")?;
        let active_paths = self.active_managed_worktree_paths()?;
        let now = Utc::now();
        let mut entries = Vec::new();

        for worktree in git.list_linked_worktrees()? {
            if !is_managed_goal_worktree(&worktree.path, &managed_root) {
                continue;
            }
            let mut entry = classify_worktree(
                &git,
                &worktree,
                &goals,
                &active_paths,
                now,
                options.older_than_seconds,
                &configured_generated_paths,
            );
            if options.apply && entry.eligible {
                match remove_generated_paths(&worktree.path, &entry.generated_paths) {
                    Ok(removed) => entry.generated_paths_removed = removed,
                    Err(error) => {
                        entry.eligible = false;
                        entry.reason = "generated_cleanup_failed".to_string();
                        entry.error = Some(error.to_string());
                    }
                }
                if entry.eligible {
                    match git.worktree_ignored_paths(&worktree.path) {
                        Ok(paths) if paths.is_empty() => {
                            match git.remove_worktree(&worktree.path, false) {
                                Ok(()) => entry.removed = true,
                                Err(error) => {
                                    entry.eligible = false;
                                    entry.reason = "removal_failed".to_string();
                                    entry.error = Some(error.to_string());
                                }
                            }
                        }
                        Ok(_) => {
                            entry.eligible = false;
                            entry.reason = "ignored_worktree".to_string();
                        }
                        Err(error) => {
                            entry.eligible = false;
                            entry.reason = "inspection_failed".to_string();
                            entry.error = Some(error.to_string());
                        }
                    }
                }
            }
            entries.push(entry);
        }

        let inspected = entries.len();
        let eligible = entries.iter().filter(|entry| entry.eligible).count();
        let removed = entries.iter().filter(|entry| entry.removed).count();
        let failed = entries.iter().filter(|entry| entry.error.is_some()).count();
        Ok(WorktreeCleanupReport {
            target_root: self.target_root.display().to_string(),
            apply: options.apply,
            older_than_seconds: options.older_than_seconds,
            inspected,
            eligible,
            removed,
            preserved: inspected.saturating_sub(removed),
            failed,
            branches_deleted: 0,
            entries,
        })
    }

    fn active_managed_worktree_paths(&self) -> RefineResult<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for process_root in [self.runtime_root.clone(), self.runtime_root.join("agents")] {
            for process in FileProcessSupervisor::new(process_root).list()? {
                collect_process_worktree_paths(&process, &mut paths);
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

fn classify_worktree(
    git: &FileGitWorktreeService,
    worktree: &GitLinkedWorktree,
    goals: &BTreeMap<String, crate::model::goal::GoalIndexProjection>,
    active_paths: &[PathBuf],
    now: DateTime<Utc>,
    older_than_seconds: u64,
    configured_generated_paths: &[PathBuf],
) -> WorktreeCleanupEntry {
    let mut entry = WorktreeCleanupEntry {
        path: worktree.path.display().to_string(),
        branch: worktree.branch.clone(),
        goal_id: None,
        goal_status: None,
        eligible: false,
        removed: false,
        reason: "goal_not_found".to_string(),
        generated_paths: Vec::new(),
        generated_paths_removed: 0,
        error: None,
    };
    let Some(branch) = worktree.branch.as_deref() else {
        entry.reason = "detached_worktree".to_string();
        return entry;
    };
    let matches = goals
        .values()
        .filter(|goal| {
            goal.branch_name.as_deref() == Some(branch)
                || branch_component_contains_goal_id(branch, &goal.id)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        entry.reason = if matches.is_empty() {
            "goal_not_found"
        } else {
            "ambiguous_goal"
        }
        .to_string();
        return entry;
    }
    let goal = matches[0];
    entry.goal_id = Some(goal.id.clone());
    entry.goal_status = Some(goal.status.as_str().to_string());
    if !matches!(goal.status, GoalStatus::Done | GoalStatus::Cancelled) {
        entry.reason = "goal_not_terminal".to_string();
        return entry;
    }
    if goal_is_too_recent(&goal.updated, now, older_than_seconds) {
        entry.reason = "retention_window".to_string();
        return entry;
    }
    if active_paths
        .iter()
        .any(|active| same_or_descendant(active, &worktree.path))
    {
        entry.reason = "active_process".to_string();
        return entry;
    }
    match git.worktree_is_clean(&worktree.path) {
        Ok(true) => {}
        Ok(false) => entry.reason = "dirty_worktree".to_string(),
        Err(error) => {
            entry.reason = "inspection_failed".to_string();
            entry.error = Some(error.to_string());
        }
    }
    if entry.reason == "dirty_worktree" || entry.error.is_some() {
        return entry;
    }
    let generated_roots = existing_generated_paths(&worktree.path, configured_generated_paths);
    match git.worktree_ignored_paths(&worktree.path) {
        Ok(ignored)
            if ignored
                .iter()
                .all(|path| ignored_path_is_generated(path, &generated_roots)) =>
        {
            // Remove the exact ignored entries Git reported, not an allowed
            // ancestor that could also contain tracked files.
            entry.generated_paths = ignored;
            entry.eligible = true;
            entry.reason = "eligible".to_string();
        }
        Ok(_) => entry.reason = "ignored_worktree".to_string(),
        Err(error) => {
            entry.reason = "inspection_failed".to_string();
            entry.error = Some(error.to_string());
        }
    }
    entry
}

fn configured_generated_paths(settings: &serde_json::Map<String, Value>) -> Vec<PathBuf> {
    settings
        .get("worktree_cleanup_generated_paths")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split([',', '\n'])
        .filter_map(valid_generated_relative_path)
        .collect()
}

fn valid_generated_relative_path(raw: &str) -> Option<PathBuf> {
    let path = Path::new(raw.trim());
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

fn existing_generated_paths(worktree: &Path, configured: &[PathBuf]) -> Vec<String> {
    let mut candidates = configured.to_vec();
    if worktree.join("Cargo.toml").is_file() {
        candidates.push(PathBuf::from("target"));
    }
    if worktree.join("xtask/Cargo.toml").is_file() {
        candidates.push(PathBuf::from("xtask/target"));
    }
    if worktree.join("package.json").is_file() {
        candidates.push(PathBuf::from("node_modules"));
    }
    candidates.sort();
    candidates.dedup();
    candidates
        .into_iter()
        .filter(|relative| worktree.join(relative).exists())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn ignored_path_is_generated(ignored: &str, generated: &[String]) -> bool {
    generated.iter().any(|path| {
        ignored == path
            || ignored
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn remove_generated_paths(worktree: &Path, generated: &[String]) -> RefineResult<usize> {
    let mut removed = 0;
    for relative in generated {
        let Some(relative) = valid_generated_relative_path(relative) else {
            continue;
        };
        let path = worktree.join(relative);
        if !path.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect generated path {}: {error}",
                path.display()
            ))
        })?;
        let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        result.map_err(|error| {
            RefineError::Io(format!(
                "failed to remove generated path {}: {error}",
                path.display()
            ))
        })?;
        removed += 1;
    }
    Ok(removed)
}

fn branch_component_contains_goal_id(branch: &str, goal_id: &str) -> bool {
    branch.split('/').any(|component| component == goal_id)
        || branch.split('-').any(|component| component == goal_id)
}

fn goal_is_too_recent(updated: &str, now: DateTime<Utc>, older_than_seconds: u64) -> bool {
    if older_than_seconds == 0 {
        return false;
    }
    let Ok(updated) = DateTime::parse_from_rfc3339(updated) else {
        return true;
    };
    now.signed_duration_since(updated.with_timezone(&Utc))
        .num_seconds()
        < i64::try_from(older_than_seconds).unwrap_or(i64::MAX)
}

fn collect_process_worktree_paths(process: &ManagedProcess, paths: &mut Vec<PathBuf>) {
    let Some(details) = process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
    else {
        return;
    };
    collect_named_paths(&details, None, paths);
}

fn collect_named_paths(value: &Value, key: Option<&str>, paths: &mut Vec<PathBuf>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                collect_named_paths(value, Some(key), paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_named_paths(value, key, paths);
            }
        }
        Value::String(path)
            if matches!(key, Some("path" | "cwd" | "worktree_path" | "target_root"))
                && !path.trim().is_empty() =>
        {
            paths.push(PathBuf::from(path));
        }
        _ => {}
    }
}

fn same_or_descendant(candidate: &Path, worktree: &Path) -> bool {
    let candidate = fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    let worktree = fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    candidate == worktree || candidate.starts_with(&worktree)
}

fn is_managed_goal_worktree(path: &Path, managed_root: &Path) -> bool {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let managed_root =
        fs::canonicalize(managed_root).unwrap_or_else(|_| managed_root.to_path_buf());
    path != managed_root && path.starts_with(managed_root)
}

pub fn automatic_cleanup_delay_seconds(settings: &serde_json::Map<String, Value>) -> Option<u64> {
    settings
        .get("worktree_cleanup_after_seconds")
        .and_then(Value::as_str)
        .unwrap_or("0")
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|value| u64::try_from(value).ok())
}

pub fn cleanup_failure_summary(report: &WorktreeCleanupReport) -> Option<Value> {
    (report.failed > 0).then(|| {
        json!({
            "failed": report.failed,
            "entries": report.entries.iter().filter(|entry| entry.error.is_some()).collect::<Vec<_>>()
        })
    })
}

#[cfg(test)]
mod tests;
