use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::infrastructure::observability::activity::ACTIVITY_LOG_FILE;
use crate::infrastructure::observability::logs::{FileLogService, goal_logs_root};
use crate::infrastructure::storage::project_layout::target_root_for_refine_dir;
use crate::model::feature::{FeatureIndexProjection, FeatureRollup, compare_feature_goal_order};
use crate::model::goal::GoalIndexProjection;
use crate::model::log::{ActivityEntry, RoundLogEntry};
use crate::model::mission::{MissionCriteriaSummary, MissionIndexProjection, MissionStatus};
use crate::model::workflow::GoalStatus;

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

/// In-memory tier of the snapshot cache, shared by every consumer in the
/// process — web handlers, workflow services, and CLI paths alike. Validated
/// by the same source fingerprints as the disk snapshot, so a hit costs one
/// stat per source instead of re-reading and re-parsing the snapshot file.
/// Keyed by refine dir + cache dir; bounded by the projects opened in-process.
static SNAPSHOT_MEMORY_CACHE: OnceLock<Mutex<BTreeMap<String, Arc<ProjectionSnapshot>>>> =
    OnceLock::new();

fn cached_snapshot(key: &str) -> Option<Arc<ProjectionSnapshot>> {
    SNAPSHOT_MEMORY_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(key).cloned())
}

fn store_cached_snapshot(key: String, snapshot: Arc<ProjectionSnapshot>) {
    if let Ok(mut cache) = SNAPSHOT_MEMORY_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
    {
        cache.insert(key, snapshot);
    }
}

/// Builds and caches a disposable read model of project Goals, Features, and activity.
///
/// The files under `refine_dir` remain authoritative. A persisted
/// [`ProjectionSnapshot`] is only a cache and can always be rebuilt from those sources.
#[derive(Clone, Debug)]
pub struct FileProjectProjectionStore {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

mod activity;
mod core;
mod features;
mod goals;
mod helpers;
mod incremental;
mod missions;
mod projection_store;

use helpers::*;
