use std::fs::{File, OpenOptions};
use std::path::Path;

use fs2::FileExt;

use crate::application::workflow::governance::integration::transaction::{
    finish_integrated_target_transaction, open_integrated_target_transaction,
};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::worktrees::FileGitWorktreeService;

const INTEGRATED_TARGET_RECONCILIATION_LOCK: &str = "refine-reconciliation.lock";

/// A repository-wide lease for integration, build, Quality, and settlement of one Goal at a time.
///
/// The flock serializes the merge→CAS→push ordering, the integration-worktree lifecycle, and
/// interrupted-transaction recovery across processes, while Git commands inside the lane retain
/// normal coordination through the short-lived repository Git lock. The durable transaction
/// lifecycle itself — the ownership marker, checkout-sync window, and interruption recovery — is
/// product state owned by `governance_integration::transaction`; this lease only serializes
/// access and delegates into it.
pub(crate) struct IntegratedTargetWorkflowLease {
    file: File,
    target_root: std::path::PathBuf,
    finished: bool,
}

impl IntegratedTargetWorkflowLease {
    pub(crate) fn acquire(
        target_root: &Path,
        goal_id: &str,
        round_idx: usize,
    ) -> RefineResult<Self> {
        let git = FileGitWorktreeService::new(target_root);
        let path = git.git_path(INTEGRATED_TARGET_RECONCILIATION_LOCK)?;
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
        open_integrated_target_transaction(target_root, goal_id, round_idx)?;
        Ok(Self {
            file,
            target_root: target_root.to_path_buf(),
            finished: false,
        })
    }

    pub(crate) fn finish(&mut self) -> RefineResult<()> {
        if self.finished {
            return Ok(());
        }
        finish_integrated_target_transaction(&self.target_root)?;
        self.finished = true;
        Ok(())
    }

    pub(crate) fn finish_if_clean(&mut self) {
        let _ = self.finish();
    }
}

impl Drop for IntegratedTargetWorkflowLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
