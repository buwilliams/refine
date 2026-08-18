mod already_merged;
mod candidate_refresh;
mod disruption;
mod execution_ownership;
mod failure_settlement;
mod governance;
mod planning_repair;
mod quality_base_refresh;
mod quality_recovery;
mod worktree_resume;

use super::*;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{FileGovernanceService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::product::work_items::FileWorkItemService;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Points the smoke-ai provider at a fixture script for the duration of a test.
/// Callers must hold `smoke_ai_env_lock` first.
struct SmokeAiGuard(Option<std::ffi::OsString>);

impl SmokeAiGuard {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", path) };
        Self(previous)
    }
}

impl Drop for SmokeAiGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.0.take() {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }
    }
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

fn git_output(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
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

#[test]
fn workflow_pass_reports_only_projection_mutations() {
    let idle = WorkflowPassResult {
        promoted: 0,
        steps: Vec::new(),
    };
    let promoted = WorkflowPassResult {
        promoted: 1,
        steps: Vec::new(),
    };

    assert!(!idle.changed_projection());
    assert!(promoted.changed_projection());
}
