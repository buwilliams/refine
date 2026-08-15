use super::*;

mod contracts;
mod hydration;
mod inspection;
mod service;
mod storage;
#[cfg(test)]
mod test_hooks;

pub use contracts::*;
#[cfg(test)]
pub(super) use hydration::hydrate_remote_with_recovery_cas;
#[cfg(not(test))]
use hydration::*;
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

struct DisposableCheckout {
    path: PathBuf,
}

impl Drop for DisposableCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn stale_recovery(reason: &str) -> RefineError {
    RefineError::Conflict(format!(
        "State recovery preview is stale because {reason}; run a new read-only preview."
    ))
}
