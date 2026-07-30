use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{ErrorKind, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
#[cfg(test)]
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::host::project_layout::{
    git_common_dir, prepare_refine_dir, state_worktree_for_target_root,
};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::ActiveGoalIndex;

const PUSH_RETRY_LIMIT: usize = 3;
const PUSH_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const REFINE_STATE_BRANCH: &str = "refine/state";
/// How long to wait for the repository lock before reporting contention.
///
/// Longer than the Git stall budget on purpose: a legitimately slow operation
/// that keeps reporting progress must be allowed to finish rather than have its
/// waiters give up underneath it, while a wedged one is stopped by its own
/// budget and releases the lock well inside this window.
const REPOSITORY_LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(600);
const REPOSITORY_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const REFINE_STATE_REF: &str = "refs/heads/refine/state";
const DEFAULT_REMOTE: &str = "origin";
const STATE_BASELINE_FILE: &str = "refine-state-baseline.json";
static REPOSITORY_GIT_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
static STATE_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

mod git_commands;
mod locks;
mod service;
mod state_codec;
mod state_files;
mod state_worktree;

pub use locks::with_repository_git_lock;

use locks::*;
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
    /// The repository is temporarily unsafe or busy. The reconciler should retry
    /// without requiring user action.
    pub deferred: bool,
}

#[derive(Clone, Debug)]
pub struct FileGitSyncService {
    pub target_root: PathBuf,
    pub runtime_root: PathBuf,
}

#[cfg(test)]
mod tests;
