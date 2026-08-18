use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::application::persistence_sync::conflict_reports::{
    StateSyncConflictPath, StateSyncConflictReport, conflict_path_summary,
};
use crate::application::persistence_sync::state::*;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::locks::with_repository_git_lock;
use crate::infrastructure::storage::project_layout::git_common_dir;

mod contracts;
mod inspection;
mod run;

pub use contracts::*;
pub use run::StateRecoveryRunPolicy;

/// Losing sides of a terminal recovery stay reachable as merge parents; this
/// names anything displaced without a parent (a joining node's live store) so
/// operators can find it by ref.
pub(crate) const RETAINED_REF_PREFIX: &str = "refs/refine/retained";

struct DisposableCheckout {
    path: PathBuf,
}

impl Drop for DisposableCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn raced_recovery(reason: &str) -> RefineError {
    RefineError::StateRecoveryConflict {
        reason: crate::error::StateRecoveryConflictReason::StateMoved,
        message: format!("State recovery lost a race because {reason}; rerun the command."),
    }
}

impl FileGitSyncService {
    fn disposable_checkout(&self, label: &str) -> RefineResult<DisposableCheckout> {
        let path = std::env::temp_dir().join(format!(
            "refine-state-recovery-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        if path.exists() {
            return Err(RefineError::Conflict(format!(
                "disposable recovery path already exists: {}",
                path.display()
            )));
        }
        Ok(DisposableCheckout { path })
    }

    /// Commit the live durable state in a disposable repository, parentless,
    /// and retain it in the target repository by ref. Used when a losing live
    /// side is not otherwise reachable from any commit (a contested join).
    pub(crate) fn retain_live_snapshot(
        &self,
        live_refine: &std::path::Path,
        label: &str,
    ) -> RefineResult<String> {
        let checkout = self.disposable_checkout("retain-live")?;
        fs::create_dir_all(&checkout.path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create retention snapshot {}: {error}",
                checkout.path.display()
            ))
        })?;
        let path = checkout.path.display().to_string();
        self.git_checked(&["init", "-q", "--", &path])?;
        replace_live_durable_state(live_refine, &checkout.path.join(".refine"))?;
        self.git_at_checked(&checkout.path, &["add", "-f", "-A", "--", ".refine"])?;
        self.git_at_checked(
            &checkout.path,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "Retain pre-recovery Refine live state",
            ],
        )?;
        let short = &label[..label.len().min(12)];
        let tree = self.git_at_stdout(&checkout.path, &["rev-parse", "HEAD^{tree}"])?;
        // An interrupted recovery reruns this retention against the same
        // remote head. Reuse the existing ref when it already holds this
        // exact live tree; otherwise pick a fresh suffixed name — never
        // overwrite a prior retention, which may hold live content the
        // interrupted attempt has since displaced.
        let base_name = format!("{RETAINED_REF_PREFIX}/live-{short}");
        let mut retained_ref = base_name.clone();
        let mut suffix = 2usize;
        loop {
            let existing =
                self.git(&["rev-parse", "--verify", &format!("{retained_ref}^{{tree}}")])?;
            if !existing.success {
                break;
            }
            if String::from_utf8_lossy(&existing.stdout).trim() == tree {
                return Ok(retained_ref);
            }
            retained_ref = format!("{base_name}-{suffix}");
            suffix += 1;
        }
        self.git_checked(&["fetch", "--no-tags", &path, &format!("HEAD:{retained_ref}")])?;
        Ok(retained_ref)
    }

    pub(crate) fn reject_foreign_git_operation(&self) -> RefineResult<()> {
        let common = git_common_dir(&self.target_root)?;
        let markers = [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "rebase-apply",
            "rebase-merge",
            "sequencer",
        ];
        if let Some(marker) = markers.iter().find(|marker| common.join(marker).exists()) {
            return Err(RefineError::Conflict(format!(
                "Git operation marker {marker} is active; recovery was not started."
            )));
        }
        Ok(())
    }
}
