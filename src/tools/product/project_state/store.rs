use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::model::feature::{FeatureIndexProjection, FeatureRollup, compare_feature_goal_order};
use crate::model::goal::GoalIndexProjection;
use crate::model::log::{ActivityEntry, RoundLogEntry};
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::project_layout::target_root_for_refine_dir;
use crate::tools::observability::activity::ACTIVITY_LOG_FILE;
use crate::tools::observability::logs::FileLogService;

use super::helpers::*;
use super::types::*;

static PROJECTION_SNAPSHOT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
/// How many round-log entries one Goal may contribute to the projection.
/// Bounds a single noisy Goal — a retry loop can append without limit.
pub(crate) const PROJECTED_ACTIVITY_PER_GOAL_LIMIT: usize = 50;
/// How many round-log entries the projection holds across all Goals. Keeps
/// resident activity independent of both Goal count and failure volume; the
/// consumers are a fifty-item dashboard list, a two-hundred-entry event feed,
/// and a paged view that reads recent-first.
const PROJECTED_ACTIVITY_LIMIT: usize = 5_000;
#[cfg(test)]
static PROJECTION_REBUILD_COUNTS: OnceLock<Mutex<BTreeMap<PathBuf, u64>>> = OnceLock::new();

pub trait ProjectStateStore {
    fn initialize(&self) -> RefineResult<()>;
    fn load_projection_snapshot(
        &self,
        cache_dir: &Path,
    ) -> RefineResult<Option<ProjectionSnapshot>>;
    fn persist_projection_snapshot(
        &self,
        cache_dir: &Path,
        snapshot: &ProjectionSnapshot,
    ) -> RefineResult<()>;
    fn rebuild_projection(&self) -> RefineResult<ProjectionSnapshot>;
}

#[derive(Clone, Debug)]
pub struct FileProjectStateStore {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

mod activity;
mod core;
mod features;
mod goals;
mod helpers;
mod projection_store;

use helpers::*;
