use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{ErrorKind, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;

use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::projects::projection::ActiveGoalIndex;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::process::supervisor::coordination::{
    record_lock_key, replace_file_durably, with_record_lock,
};
#[cfg(test)]
use crate::infrastructure::storage::project_layout::refine_dir_for_target_root;
use crate::infrastructure::storage::project_layout::{
    git_common_dir, prepare_refine_dir, state_worktree_for_target_root,
};

const PUSH_RETRY_LIMIT: usize = 3;
const PUSH_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const REFINE_STATE_BRANCH: &str = "refine/state";
pub(crate) const REFINE_STATE_REF: &str = "refs/heads/refine/state";
pub(crate) const DEFAULT_REMOTE: &str = "origin";
pub(crate) static STATE_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) mod service;
mod state_codec;
mod state_files;
mod state_worktree;

use crate::application::persistence_sync::conflict_reports::conflict_path_summary;
#[cfg(test)]
use crate::application::persistence_sync::conflict_reports::latest_state_sync_conflict_report;
use crate::application::persistence_sync::conflict_reports::{
    StateSyncConflictPath, StateSyncConflictPhase,
};
use crate::application::persistence_sync::recovery::{
    StateRecoveryAuthority, StateRecoveryDecision,
};
#[cfg(test)]
use crate::application::persistence_sync::recovery::{
    StateRecoveryOverride, StateRecoveryRunPolicy,
};

use crate::infrastructure::git::repository::FileGitRepository;
use crate::infrastructure::git::repository::{GitCommandOutput, command_failed};
pub(crate) use state_codec::*;
pub(crate) use state_files::*;

#[derive(Clone, Copy)]
pub(crate) enum GitFetchScope {
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

pub(crate) fn deferred(detail: &str) -> GitSyncResult {
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
    repository: FileGitRepository,
    /// Whether this entry point may call out to the installed agent for
    /// conflicts the deterministic ladder cannot settle. Off by default so
    /// bare constructions (fixtures, harnesses, read paths) keep the
    /// fail-closed ladder; the daemon runner and the CLI sync surface opt in
    /// with [`FileGitSyncService::with_agent_resolution`]. A resolver
    /// override installed for the target root engages regardless.
    pub(crate) agent_resolution: bool,
}

impl std::ops::Deref for FileGitSyncService {
    type Target = FileGitRepository;

    fn deref(&self) -> &Self::Target {
        &self.repository
    }
}

#[cfg(test)]
mod tests;
