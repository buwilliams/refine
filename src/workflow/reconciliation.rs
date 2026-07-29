use std::fs::{File, OpenOptions};
use std::path::Path;

use fs2::FileExt;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::FileGitWorktreeService;

const INTEGRATED_TARGET_RECONCILIATION_LOCK: &str = "refine-reconciliation.lock";

/// A repository-wide lease for quality and settlement against the integrated target.
///
/// Already-merged reconciliation deliberately evaluates the shared target worktree and may
/// revert its merge on failure. A separate lock from the short-lived repository Git lock lets
/// the lease span supervised Quality while Git operations inside that critical section continue
/// to use their normal coordination.
pub(super) struct IntegratedTargetReconciliationLease {
    file: File,
}

impl IntegratedTargetReconciliationLease {
    pub(super) fn acquire(target_root: &Path) -> RefineResult<Self> {
        let path = FileGitWorktreeService::new(target_root)
            .git_path(INTEGRATED_TARGET_RECONCILIATION_LOCK)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open integrated-target reconciliation lock {}: {error}",
                    path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to acquire integrated-target reconciliation lock for {}: {error}",
                target_root.display()
            ))
        })?;
        Ok(Self { file })
    }
}

impl Drop for IntegratedTargetReconciliationLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
