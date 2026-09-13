use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::record_lock_key;
use crate::infrastructure::process::supervisor::coordination::replace_file_durably;
use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;

use super::store::FileProjectProjectionStore;

pub const ACTIVE_GOALS_FILE: &str = "runtime/active-goals.jsonl";
/// Rewrite the log once it carries more than this multiple of the records it
/// actually describes. Every write appends, so a long-lived Goal accumulates one
/// superseded line per transition; without compaction, replay cost would grow
/// with a project's history rather than with its live work.
const COMPACTION_LINE_RATIO: usize = 3;
/// Below this, replay is cheap enough that rewriting costs more than it saves.
const COMPACTION_MINIMUM_LINES: usize = 512;
mod recovery;

/// One appended decision about a Goal.
#[derive(Debug, Deserialize, Serialize)]
struct ActiveGoalRecord {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    goal: Option<Box<GoalIndexProjection>>,
    #[serde(default, skip_serializing_if = "is_false")]
    removed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<GoalProjectionError>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GoalProjectionError {
    path: PathBuf,
    message: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Goals that still hold the scheduler's attention.
///
/// Scheduling decisions never need a project's whole history. Feature ordering
/// is decided by Goals that still hold their Feature's queue, and priority by
/// Goals still waiting to run; a Goal that reached Review, Done, or
/// Cancelled influences neither. Terminal Goals remain here only while they
/// retain pending Event delivery, so the same index lets the daemon reconstruct
/// dispatch queues without scanning completed history. In a mature project
/// settled Goals are the overwhelming
/// majority, so an index of the rest is bounded by work in flight rather than by
/// how much work has ever been done.
///
/// This is what decouples scheduler memory from project size. Loading the full
/// projection to schedule meant holding every Goal ever created, which does not
/// survive five million of them.
#[derive(Clone, Debug, Default)]
pub struct ActiveGoalIndex {
    goals: BTreeMap<String, GoalIndexProjection>,
    errors: BTreeMap<String, GoalProjectionError>,
}

impl ActiveGoalIndex {
    pub fn path(refine_dir: &Path) -> PathBuf {
        refine_dir.join(ACTIVE_GOALS_FILE)
    }

    /// Whether a Goal in this status still affects scheduling.
    ///
    /// Mirrors the eligibility rules exactly: Review, Done, and Cancelled
    /// release a Feature's ordering queue and are not schedulable, so nothing
    /// the scheduler computes can depend on them. Backlog and Failed do still
    /// hold their Feature's queue, so both remain resident despite not being
    /// directly runnable.
    pub fn holds_scheduler_attention(status: &GoalStatus) -> bool {
        !matches!(
            status,
            GoalStatus::Review | GoalStatus::Done | GoalStatus::Cancelled
        )
    }

    fn pending_work(goal: &serde_json::Value) -> bool {
        goal["pending_event_dispatches"]
            .as_object()
            .is_some_and(|pending| !pending.is_empty())
            || goal["pending_event_transition"]["state"] == "pending"
            || goal["pending_workflow_outcome"]["state"] == "pending"
            || goal["workflow_integration_control"]["state"] == "pending"
    }

    /// The concrete iterator type rather than `impl Iterator`, because
    /// eligibility walks the set more than once and needs to clone it.
    pub fn goals(&self) -> std::collections::btree_map::Values<'_, String, GoalIndexProjection> {
        self.goals.values()
    }

    pub fn len(&self) -> usize {
        self.goals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.goals.is_empty()
    }

    /// Source records needing recovery remain visible even when they cannot be scheduled.
    pub fn recovery_errors(&self) -> impl Iterator<Item = (&str, &str)> {
        self.errors
            .iter()
            .map(|(id, error)| (id.as_str(), error.message.as_str()))
    }

    /// Read the index, reconstructing it from Goal records if it is absent.
    ///
    /// Reconstruction streams: each record is projected and then dropped unless
    /// it is still active, so even a first run on a very large project holds
    /// only the live set rather than the whole corpus.
    pub fn load_or_rebuild(refine_dir: &Path) -> RefineResult<Self> {
        Self::reconcile_pending(refine_dir)?;
        let mut index = Self::with_index_lock(refine_dir, || {
            if let Some(index) = Self::load(refine_dir)? {
                return Ok(index);
            }
            let index = Self::rebuild(refine_dir)?;
            index.persist_locked(refine_dir)?;
            Ok(index)
        })?;
        // Retry only known damaged sources. A repaired source can rejoin the
        // scheduler without a full historical scan or a daemon restart.
        if !index.errors.is_empty() {
            for error in index.errors.values() {
                let path = refine_dir.join(&error.path);
                let _ = Self::record_goal(refine_dir, &path);
            }
            index = Self::with_index_lock(refine_dir, || {
                Self::load(refine_dir)?.ok_or_else(|| {
                    RefineError::Io("active Goal index disappeared during recovery".into())
                })
            })?;
        }
        Ok(index)
    }

    /// Materialize the index if it is missing, without keeping it.
    ///
    /// Reconstruction reads every Goal record. Callers that hold a lock other
    /// work waits on use this first, so the expensive path runs outside that
    /// lock and the load inside it only reads the live set.
    pub fn ensure_built(refine_dir: &Path) -> RefineResult<()> {
        if Self::path(refine_dir).exists() {
            return Ok(());
        }
        Self::load_or_rebuild(refine_dir).map(|_| ())
    }

    fn load(refine_dir: &Path) -> RefineResult<Option<Self>> {
        let path = Self::path(refine_dir);
        let Ok(file) = File::open(&path) else {
            return Ok(None);
        };
        let mut goals = BTreeMap::new();
        let mut errors = BTreeMap::new();
        let mut lines = 0usize;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read active Goal index {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            lines += 1;
            // A malformed line is a damaged derived file, not a damaged project.
            // Reconstructing costs one pass; trusting a partial replay would
            // silently drop live work from scheduling.
            let Ok(record) = serde_json::from_str::<ActiveGoalRecord>(&line) else {
                return Ok(None);
            };
            if let Some(error) = record.error {
                goals.remove(&record.id);
                errors.insert(record.id, error);
                continue;
            }
            errors.remove(&record.id);
            match record.goal {
                Some(goal) if !record.removed => {
                    goals.insert(record.id, *goal);
                }
                _ => {
                    goals.remove(&record.id);
                }
            }
        }
        let index = Self { goals, errors };
        if lines > COMPACTION_MINIMUM_LINES
            && lines > (index.goals.len() + index.errors.len()) * COMPACTION_LINE_RATIO
        {
            index.persist_locked(refine_dir)?;
        }
        Ok(Some(index))
    }

    /// Project every Goal record, keeping only those that still matter.
    pub fn rebuild(refine_dir: &Path) -> RefineResult<Self> {
        let store = FileProjectProjectionStore::new(refine_dir);
        let mut goals = BTreeMap::new();
        let mut errors = BTreeMap::new();
        for path in FileProjectProjectionStore::collect_goal_record_paths(refine_dir)? {
            let source = fs::read(&path)
                .map_err(|error| RefineError::Io(error.to_string()))
                .and_then(|bytes| {
                    serde_json::from_slice::<serde_json::Value>(&bytes)
                        .map_err(|error| RefineError::Serialization(error.to_string()))
                });
            let projection = match source
                .as_ref()
                .map_err(|error| RefineError::Io(error.to_string()))
                .and_then(|value| store.project_goal_value(&path, value))
            {
                Ok(Some(projection)) => projection,
                result => {
                    let error = Self::projection_error(refine_dir, &path, result.err());
                    eprintln!("refine Goal discovery: {}", error.message);
                    errors.insert(record_lock_key(&path), error);
                    continue;
                }
            };
            if Self::holds_scheduler_attention(&projection.goal.status)
                || source.as_ref().is_ok_and(Self::pending_work)
            {
                goals.insert(projection.goal.id.clone(), projection.goal);
            }
        }
        Ok(Self { goals, errors })
    }

    /// Rewrite the log so it contains exactly the live set.
    pub fn persist(&self, refine_dir: &Path) -> RefineResult<()> {
        Self::with_index_lock(refine_dir, || self.persist_locked(refine_dir))
    }

    fn persist_locked(&self, refine_dir: &Path) -> RefineResult<()> {
        let path = Self::path(refine_dir);
        let mut encoded = String::new();
        for goal in self.goals.values() {
            let record = ActiveGoalRecord {
                id: goal.id.clone(),
                goal: Some(Box::new(goal.clone())),
                removed: false,
                error: None,
            };
            let line = serde_json::to_string(&record).map_err(|error| {
                RefineError::Serialization(format!("failed to encode active Goal record: {error}"))
            })?;
            encoded.push_str(&line);
            encoded.push('\n');
        }
        for (id, error) in &self.errors {
            let record = ActiveGoalRecord {
                id: id.clone(),
                goal: None,
                removed: false,
                error: Some(error.clone()),
            };
            encoded.push_str(
                &serde_json::to_string(&record)
                    .map_err(|error| RefineError::Serialization(error.to_string()))?,
            );
            encoded.push('\n');
        }
        replace_file_durably(&path, encoded.as_bytes())
    }

    /// Record the current state of one Goal.
    ///
    /// Called after the Goal record is written, so the index follows the source
    /// of truth rather than predicting it. Write-through is what keeps this
    /// affordable: the alternative is comparing every Goal file against the
    /// index to discover which moved, which costs a stat per Goal on every
    /// scheduling pass and is exactly the scan this exists to avoid.
    pub fn record_goal(refine_dir: &Path, goal_path: &Path) -> RefineResult<()> {
        Self::with_index_lock(refine_dir, || {
            Self::record_goal_locked(refine_dir, goal_path)
        })
    }

    fn record_goal_locked(refine_dir: &Path, goal_path: &Path) -> RefineResult<()> {
        let store = FileProjectProjectionStore::new(refine_dir);
        let source = fs::read(goal_path)
            .map_err(|error| RefineError::Io(error.to_string()))
            .and_then(|bytes| {
                serde_json::from_slice::<serde_json::Value>(&bytes)
                    .map_err(|error| RefineError::Serialization(error.to_string()))
            });
        let record = match source
            .as_ref()
            .map_err(|error| RefineError::Io(error.to_string()))
            .and_then(|value| store.project_goal_value(goal_path, value))
        {
            Ok(Some(projection))
                if Self::holds_scheduler_attention(&projection.goal.status)
                    || source.as_ref().is_ok_and(Self::pending_work) =>
            {
                ActiveGoalRecord {
                    id: projection.goal.id.clone(),
                    goal: Some(Box::new(projection.goal)),
                    removed: false,
                    error: None,
                }
            }
            Ok(Some(projection)) => ActiveGoalRecord {
                id: projection.goal.id,
                goal: None,
                removed: true,
                error: None,
            },
            _ if matches!(fs::symlink_metadata(goal_path), Err(error) if error.kind() == std::io::ErrorKind::NotFound) => {
                ActiveGoalRecord {
                    id: record_lock_key(goal_path),
                    goal: None,
                    removed: true,
                    error: None,
                }
            }
            result => ActiveGoalRecord {
                id: record_lock_key(goal_path),
                goal: None,
                removed: false,
                error: Some(Self::projection_error(refine_dir, goal_path, result.err())),
            },
        };
        Self::append_locked(refine_dir, &record)
    }

    /// Record that a Goal no longer exists.
    pub fn forget_goal(refine_dir: &Path, goal_id: &str) -> RefineResult<()> {
        Self::append(
            refine_dir,
            &ActiveGoalRecord {
                id: goal_id.to_string(),
                goal: None,
                removed: true,
                error: None,
            },
        )
    }

    fn append(refine_dir: &Path, record: &ActiveGoalRecord) -> RefineResult<()> {
        Self::with_index_lock(refine_dir, || Self::append_locked(refine_dir, record))
    }

    fn append_locked(refine_dir: &Path, record: &ActiveGoalRecord) -> RefineResult<()> {
        let path = Self::path(refine_dir);
        if !path.exists() {
            // An interrupted first append must not turn an absent index into a
            // valid but partial index that hides other existing Goals.
            Self::rebuild(refine_dir)?.persist_locked(refine_dir)?;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create active Goal index directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let line = serde_json::to_string(record).map_err(|error| {
            RefineError::Serialization(format!("failed to encode active Goal record: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open active Goal index {}: {error}",
                    path.display()
                ))
            })?;
        writeln!(file, "{line}")
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to append active Goal index {}: {error}",
                    path.display()
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_goal(refine_dir: &Path, id: &str, status: &str) -> PathBuf {
        let dir = refine_dir.join("goals").join(&id[..2]).join(&id[2..]);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("goal.json");
        fs::write(
            &path,
            format!(r#"{{"id":"{id}","name":"{id}","status":"{status}","rounds":[]}}"#),
        )
        .unwrap();
        path
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    // Scheduling never depends on a Goal past Review, so the index that feeds it
    // is bounded by work in flight rather than by everything the project has
    // ever contained. Backlog and Failed stay resident because they still hold
    // their Feature's ordering queue even though neither is directly runnable.
    #[test]
    fn the_index_holds_only_goals_that_still_affect_scheduling() {
        let refine_dir = unique_temp_dir("active-index-membership").join(".refine");
        for (id, status) in [
            ("GOALBACKLOG", "backlog"),
            ("GOALTODO000", "todo"),
            ("GOALPROGRES", "in-progress"),
            ("GOALREADYME", "ready-merge"),
            ("GOALFAILED0", "failed"),
            ("GOALREVIEW0", "review"),
            ("GOALDONE000", "done"),
            ("GOALCANCEL0", "cancelled"),
        ] {
            write_goal(&refine_dir, id, status);
        }

        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();

        let mut ids = index
            .goals()
            .map(|goal| goal.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "GOALBACKLOG",
                "GOALFAILED0",
                "GOALPROGRES",
                "GOALREADYME",
                "GOALTODO000",
            ]
        );
        let _ = fs::remove_dir_all(refine_dir.parent().unwrap());
    }

    // The property the five-million-Goal target rests on: the resident set
    // tracks live work, not project size. The ratio is what matters here rather
    // than the absolute count, which is why a fixture that fits in a test still
    // demonstrates it.
    #[test]
    fn resident_size_tracks_live_work_rather_than_project_size() {
        let refine_dir = unique_temp_dir("active-index-scale").join(".refine");
        const TOTAL: usize = 4_000;
        const LIVE: usize = 20;
        for index in 0..TOTAL {
            let status = if index < LIVE { "todo" } else { "done" };
            write_goal(&refine_dir, &format!("GOAL{index:07}"), status);
        }

        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();

        assert_eq!(index.len(), LIVE);
        // Reloading reads the index rather than the corpus, so the cost of a
        // scheduling pass does not grow with the completed history behind it.
        let reloaded = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
        assert_eq!(reloaded.len(), LIVE);
        let _ = fs::remove_dir_all(refine_dir.parent().unwrap());
    }

    // Write-through is what makes the index affordable: without it, discovering
    // which Goals moved means comparing every Goal file against the index on
    // every scheduling pass, which is the scan the index exists to avoid.
    #[test]
    fn recording_a_transition_moves_a_goal_in_or_out() {
        let refine_dir = unique_temp_dir("active-index-writethrough").join(".refine");
        let path = write_goal(&refine_dir, "GOALMOVING0", "todo");
        ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();

        // Leaving the live set.
        write_goal(&refine_dir, "GOALMOVING0", "done");
        ActiveGoalIndex::record_goal(&refine_dir, &path).unwrap();
        assert!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir)
                .unwrap()
                .is_empty()
        );

        // And returning to it, so a reopened Goal is scheduled again.
        write_goal(&refine_dir, "GOALMOVING0", "todo");
        ActiveGoalIndex::record_goal(&refine_dir, &path).unwrap();
        assert_eq!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap().len(),
            1
        );

        // A deleted record cannot be re-projected, so removal is explicit.
        ActiveGoalIndex::forget_goal(&refine_dir, "GOALMOVING0").unwrap();
        assert!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(refine_dir.parent().unwrap());
    }

    // A damaged derived file must not silently drop live work from scheduling.
    #[test]
    fn a_corrupt_index_is_reconstructed_from_goal_records() {
        let refine_dir = unique_temp_dir("active-index-corrupt").join(".refine");
        write_goal(&refine_dir, "GOALINTACT0", "todo");
        ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
        fs::write(ActiveGoalIndex::path(&refine_dir), "{not json\n").unwrap();

        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();

        assert_eq!(index.len(), 1);
        assert_eq!(index.goals().next().unwrap().id, "GOALINTACT0");
        let _ = fs::remove_dir_all(refine_dir.parent().unwrap());
    }

    #[test]
    fn committed_goal_is_rediscovered_after_interrupted_index_update() {
        let refine_dir = unique_temp_dir("active-index-interrupted-write").join(".refine");
        let path = write_goal(&refine_dir, "GOALMOVING0", "done");
        assert!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir)
                .unwrap()
                .is_empty()
        );
        let selected = br#"{"id":"GOALMOVING0","status":"todo","rounds":[],"workflow_revision":1}"#;
        ActiveGoalIndex::prepare_goal_write(&refine_dir, &path, Some(selected)).unwrap();

        // A reader in the prepare/replace gap must not consume the marker.
        assert!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir)
                .unwrap()
                .is_empty()
        );
        fs::write(&path, selected).unwrap();
        // The source replacement committed, but its writer died before append.
        assert_eq!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap().len(),
            1
        );
        assert_eq!(
            fs::read_dir(refine_dir.join("runtime/active-goals-pending"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(refine_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn corrupt_goal_is_visible_without_blocking_siblings_and_rejoins_after_repair() {
        let refine_dir = unique_temp_dir("active-index-corrupt-goal").join(".refine");
        write_goal(&refine_dir, "GOALINTACT0", "todo");
        let damaged = write_goal(&refine_dir, "GOALBROKEN0", "todo");
        fs::write(&damaged, "{corrupt").unwrap();
        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
        assert_eq!(index.len(), 1);
        let errors = index.recovery_errors().collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "GOALBROKEN0");
        assert!(errors[0].1.contains("needs recovery"));
        write_goal(&refine_dir, "GOALBROKEN0", "todo");
        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.recovery_errors().count(), 0);
        fs::remove_dir_all(refine_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_damaged_discovery_marker_still_identifies_the_committed_source() {
        let refine_dir = unique_temp_dir("active-index-damaged-marker").join(".refine");
        let path = write_goal(&refine_dir, "GOALMOVING0", "done");
        assert!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir)
                .unwrap()
                .is_empty()
        );
        let selected = br#"{"id":"GOALMOVING0","status":"todo","rounds":[]}"#;
        ActiveGoalIndex::prepare_goal_write(&refine_dir, &path, Some(selected)).unwrap();
        fs::write(&path, selected).unwrap();
        let marker = fs::read_dir(refine_dir.join("runtime/active-goals-pending"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(&marker, "{damaged pending index").unwrap();
        assert_eq!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap().len(),
            1
        );
        // Replaying damaged derived state cannot clear a future writer's marker.
        assert!(marker.exists());
        ActiveGoalIndex::prepare_goal_write(&refine_dir, &path, Some(selected)).unwrap();
        ActiveGoalIndex::complete_goal_write(&refine_dir, &path).unwrap();
        assert!(!marker.exists());
        fs::remove_dir_all(refine_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn inaccessible_source_path_is_a_recovery_error_not_a_deleted_goal() {
        let refine_dir = unique_temp_dir("active-index-source-path-error").join(".refine");
        let path = write_goal(&refine_dir, "GOALMOVING0", "todo");
        assert_eq!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap().len(),
            1
        );
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
        fs::write(path.parent().unwrap(), "unavailable source directory").unwrap();
        ActiveGoalIndex::record_goal(&refine_dir, &path).unwrap();
        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
        assert_eq!(index.recovery_errors().count(), 1);
        assert_eq!(index.recovery_errors().next().unwrap().0, "GOALMOVING0");
        fs::remove_file(path.parent().unwrap()).unwrap();
        write_goal(&refine_dir, "GOALMOVING0", "todo");
        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(index.recovery_errors().count(), 0);
        fs::remove_dir_all(refine_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn terminal_goals_remain_discoverable_only_while_dispatch_is_pending() {
        let refine_dir = unique_temp_dir("active-index-terminal-dispatch").join(".refine");
        let path = write_goal(&refine_dir, "GOALDONE000", "done");
        let mut goal: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for field in [
            "pending_event_transition",
            "pending_workflow_outcome",
            "workflow_integration_control",
        ] {
            goal[field] = serde_json::json!({"state":"pending"});
            fs::write(&path, serde_json::to_vec(&goal).unwrap()).unwrap();
            ActiveGoalIndex::record_goal(&refine_dir, &path).unwrap();
            assert_eq!(
                ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap().len(),
                1
            );
            goal[field]["state"] = serde_json::json!("superseded");
            fs::write(&path, serde_json::to_vec(&goal).unwrap()).unwrap();
            ActiveGoalIndex::record_goal(&refine_dir, &path).unwrap();
            assert!(
                ActiveGoalIndex::load_or_rebuild(&refine_dir)
                    .unwrap()
                    .is_empty()
            );
        }
        goal["pending_event_dispatches"] = serde_json::json!({"occurrence":{}});
        fs::write(&path, serde_json::to_vec(&goal).unwrap()).unwrap();
        ActiveGoalIndex::record_goal(&refine_dir, &path).unwrap();
        assert_eq!(
            ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap().len(),
            1
        );
        fs::remove_dir_all(refine_dir.parent().unwrap()).unwrap();
    }
}
