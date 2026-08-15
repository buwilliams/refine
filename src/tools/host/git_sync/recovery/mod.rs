use super::*;

mod contracts;
mod hydration;
mod inspection;
mod reported;
mod reported_support;
mod service;
mod storage;
#[cfg(test)]
mod test_hooks;

pub use contracts::*;
#[cfg(test)]
pub(super) use hydration::hydrate_remote_with_recovery_cas;
#[cfg(not(test))]
use hydration::*;
pub(super) use hydration::{hydrate_recovery_target_from_map, hydrate_recovery_target_with_cas};
use storage::*;
#[cfg(test)]
use test_hooks::*;
#[cfg(test)]
pub(super) use test_hooks::{
    install_after_recovery_authority_hook, install_after_recovery_baseline_hook,
};

const RECOVERY_PATH_LIMIT: usize = 100;
const RECOVERY_MANIFEST_DIR: &str = "refine-state-recoveries";
const RECOVERY_REF_PREFIX: &str = "refs/refine/state-recovery";
const REPORTED_RECOVERY_VERSION: u32 = 2;

#[cfg(test)]
pub(in crate::tools::host::git_sync) fn preview_evidence_id_for_test(
    preview: &StateRecoveryPreview,
) -> RefineResult<String> {
    storage::preview_evidence_id(preview)
}

struct DisposableCheckout {
    path: PathBuf,
}

impl Drop for DisposableCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn stale_recovery(reason: &str) -> RefineError {
    RefineError::StateRecoveryConflict {
        reason: crate::process::supervisor::errors::StateRecoveryConflictReason::StalePreview,
        message: format!(
            "State recovery preview is stale because {reason}; run a new read-only preview."
        ),
    }
}

fn git_busy_recovery() -> RefineError {
    RefineError::StateRecoveryConflict {
        reason: crate::process::supervisor::errors::StateRecoveryConflictReason::GitBusy,
        message: "Repository Git operations are busy; recovery was not started.".to_string(),
    }
}
