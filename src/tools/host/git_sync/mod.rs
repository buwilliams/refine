use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{ErrorKind, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Arc;
use std::sync::{Mutex, OnceLock, TryLockError};
use std::thread;
#[cfg(test)]
use std::time::Instant;
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;

use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::coordination::{
    record_lock_key, replace_file_durably, with_record_lock,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
#[cfg(test)]
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::host::project_layout::{
    git_common_dir, prepare_refine_dir, state_worktree_for_target_root,
};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_projection::ActiveGoalIndex;

const PUSH_RETRY_LIMIT: usize = 3;
const PUSH_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const REFINE_STATE_BRANCH: &str = "refine/state";
const REFINE_STATE_REF: &str = "refs/heads/refine/state";
const DEFAULT_REMOTE: &str = "origin";
const STATE_BASELINE_FILE: &str = "refine-state-baseline.json";
const STATE_BASELINE_REF_PREFIX: &str = "refs/refine/state-baseline";
static STATE_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

mod baseline;
#[cfg(test)]
use baseline::install_after_baseline_anchor_hook;
mod conflict_report;
mod goal_merge;
mod node_registry_merge;
mod recovery;
mod semantic_merge;
mod service;
mod state_codec;
mod state_files;
mod state_worktree;

pub use conflict_report::{
    StateSyncConflictPhase, StateSyncConflictReport, StateSyncConflictSummary,
    latest_state_sync_conflict_report,
};
use recovery::hydrate_recovery_target_from_map;
pub use recovery::{
    StateRecoveryAuthority, StateRecoveryDecision, StateRecoveryManifest, StateRecoveryOutcome,
    StateRecoveryOverride, StateRecoveryPathCounts, StateRecoveryPreview, StateRecoveryResult,
    StateRecoveryRunPolicy, StateRecoveryRunResult, StateRecoveryStage,
};
#[cfg(test)]
use recovery::{
    hydrate_remote_with_recovery_cas, install_after_recovery_authority_hook,
    install_after_recovery_baseline_hook, install_during_recovery_preview_hook,
};

use crate::tools::git::locks::{RepositoryFileLock, repository_git_lock, with_repository_git_lock};
use crate::tools::git::repo::{GitCommandOutput, command_failed};
use goal_merge::*;
use node_registry_merge::*;
use semantic_merge::BaselineFileResolution;
pub use semantic_merge::{
    BaselineReconstructionSource, StateSyncReconciliationKind, StateSyncReconciliationOutcome,
};
use state_codec::*;
use state_files::*;

#[derive(Clone, Copy)]
enum GitFetchScope {
    State,
    All,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitSyncResult {
    pub ok: bool,
    pub attempted: bool,
    pub committed: bool,
    pub pulled: bool,
    pub pushed: bool,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub detail: Option<String>,
    /// Whether this target has the configured Git remote. `None` means the
    /// operation did not reach remote discovery (for example, a lock deferral).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_configured: Option<bool>,
    /// The repository is temporarily unsafe or busy. The reconciler should retry
    /// without requiring user action.
    pub deferred: bool,
}

fn skipped(detail: &str) -> GitSyncResult {
    GitSyncResult {
        ok: true,
        detail: Some(detail.to_string()),
        ..GitSyncResult::default()
    }
}

fn deferred(detail: &str) -> GitSyncResult {
    GitSyncResult {
        ok: true,
        detail: Some(detail.to_string()),
        deferred: true,
        ..GitSyncResult::default()
    }
}

#[derive(Clone, Debug)]
pub struct StateSyncAttemptContext {
    pub id: String,
    pub source: String,
}

impl StateSyncAttemptContext {
    pub fn new(id: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
        }
    }

    fn direct() -> Self {
        Self::new(uuid::Uuid::new_v4().to_string(), "direct")
    }
}

#[derive(Clone, Debug)]
pub struct FileGitSyncService {
    pub target_root: PathBuf,
    pub runtime_root: PathBuf,
}

#[cfg(test)]
mod tests;
