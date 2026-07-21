use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::process::supervisor::errors::{RefineError, RefineResult};

pub const LEGACY_REFINE_DIR: &str = ".refine";
pub const LIVE_REFINE_STATE_DIR: &str = "refine-live-state";
pub const REFINE_STATE_WORKTREE_DIR: &str = "refine-state-worktree";

/// Locate the repository's shared Git directory.
pub fn git_common_dir(target_root: &Path) -> RefineResult<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(target_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| RefineError::Io(format!("failed to locate Git directory: {error}")))?;
    if !output.status.success() {
        return Err(RefineError::InvalidInput(format!(
            "target app {} must be a Git worktree",
            target_root.display()
        )));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Err(RefineError::Conflict(
            "Git returned an empty common directory".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    Ok(if path.is_absolute() {
        path
    } else {
        target_root.join(path)
    })
}

/// The live durable-state projection used by Refine services. Its contents
/// mirror the `.refine/` tree committed on `refine/state`, but the projection
/// itself is below the repository's Git directory, outside the primary
/// application worktree.
pub fn refine_dir_for_target_root(target_root: &Path) -> RefineResult<PathBuf> {
    #[cfg(test)]
    if !target_root.join(".git").exists() {
        return Ok(target_root.join(LEGACY_REFINE_DIR));
    }
    Ok(git_common_dir(target_root)?.join(LIVE_REFINE_STATE_DIR))
}

pub fn state_worktree_for_target_root(target_root: &Path) -> RefineResult<PathBuf> {
    Ok(git_common_dir(target_root)?.join(REFINE_STATE_WORKTREE_DIR))
}

/// Resolve the application worktree associated with either the external live
/// state directory or a legacy/test `<target>/.refine` directory.
pub fn target_root_for_refine_dir(refine_dir: &Path) -> RefineResult<PathBuf> {
    let parent = refine_dir.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "Refine state directory {} has no parent",
            refine_dir.display()
        ))
    })?;
    let name = refine_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            RefineError::InvalidInput(format!(
                "Refine state directory {} has no usable name",
                refine_dir.display()
            ))
        })?;
    if name == LEGACY_REFINE_DIR {
        return Ok(parent.to_path_buf());
    }
    if name != LIVE_REFINE_STATE_DIR {
        return Err(RefineError::InvalidInput(format!(
            "Refine state directory {} does not match the Git-owned state layout",
            refine_dir.display()
        )));
    }
    let target_root = parent.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "Refine state directory {} has no target-app parent",
            refine_dir.display()
        ))
    })?;
    Ok(target_root.to_path_buf())
}

/// Move a pre-v4 `<target>/.refine` tree into the Git-owned live-state directory.
/// This is intentionally a rename: the old path disappears atomically and no
/// second live copy remains in the application worktree.
pub fn prepare_refine_dir(target_root: &Path) -> RefineResult<PathBuf> {
    #[cfg(test)]
    if !target_root.join(".git").exists() {
        return Ok(target_root.join(LEGACY_REFINE_DIR));
    }
    let refine_dir = refine_dir_for_target_root(target_root)?;
    let legacy = target_root.join(LEGACY_REFINE_DIR);
    if legacy.exists() {
        if refine_dir.exists() {
            return Err(RefineError::Conflict(format!(
                "both legacy Refine state {} and external Refine state {} exist; reconcile them before attaching",
                legacy.display(),
                refine_dir.display()
            )));
        }
        if let Some(parent) = refine_dir.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Refine state parent {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::rename(&legacy, &refine_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to move legacy Refine state {} to {}: {error}",
                legacy.display(),
                refine_dir.display()
            ))
        })?;
    }
    let tracked = tracked_legacy_state(target_root)?;
    if !tracked.is_empty() {
        return Err(RefineError::Conflict(format!(
            "the application branch still tracks legacy .refine state ({}); commit its removal from the application branch, then attach again",
            tracked.join(", ")
        )));
    }
    Ok(refine_dir)
}

fn tracked_legacy_state(target_root: &Path) -> RefineResult<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files", "--", LEGACY_REFINE_DIR])
        .current_dir(target_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| {
            RefineError::Io(format!("failed to inspect tracked Refine state: {error}"))
        })?;
    if !output.status.success() {
        return Err(RefineError::Conflict(format!(
            "failed to inspect tracked Refine state: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToString::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn prepare_refine_dir_moves_legacy_state_out_of_primary_worktree() {
        let root = unique_temp_dir("project-layout");
        let app = root.join("app");
        fs::create_dir_all(app.join(".refine/goals/GOAL1")).unwrap();
        fs::write(app.join(".refine/goals/GOAL1/goal.json"), "{}\n").unwrap();
        git(&app, &["init", "-q"]);

        let refine_dir = prepare_refine_dir(&app).unwrap();

        assert!(!app.join(".refine").exists());
        assert!(refine_dir.join("goals/GOAL1/goal.json").exists());
        assert_eq!(refine_dir, app.join(".git/refine-live-state"));
        assert_eq!(target_root_for_refine_dir(&refine_dir).unwrap(), app);
        assert_eq!(
            state_worktree_for_target_root(&app).unwrap(),
            app.join(".git/refine-state-worktree")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepare_refine_dir_requires_legacy_state_to_leave_application_branch() {
        let root = unique_temp_dir("project-layout-tracked");
        let app = root.join("app");
        fs::create_dir_all(app.join(".refine")).unwrap();
        fs::write(app.join(".refine/refine.json"), "{}\n").unwrap();
        git(&app, &["init", "-q"]);
        git(&app, &["add", ".refine"]);

        let error = prepare_refine_dir(&app).unwrap_err();

        assert!(!app.join(".refine").exists());
        assert!(error.to_string().contains("still tracks legacy .refine"));
        fs::remove_dir_all(root).unwrap();
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
