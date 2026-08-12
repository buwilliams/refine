use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::coordination::replace_file_durably;
use crate::process::supervisor::errors::{RefineError, RefineResult};

use super::store::FileProjectProjectionStore;

pub const ACTIVE_GOALS_FILE: &str = "runtime/active-goals.jsonl";
/// Rewrite the log once it carries more than this multiple of the records it
/// actually describes. Every write appends, so a long-lived Goal accumulates one
/// superseded line per transition; without compaction, replay cost would grow
/// with a project's history rather than with its live work.
const COMPACTION_LINE_RATIO: usize = 3;
/// Below this, replay is cheap enough that rewriting costs more than it saves.
const COMPACTION_MINIMUM_LINES: usize = 512;

/// One appended decision about a Goal.
#[derive(Debug, Deserialize, Serialize)]
struct ActiveGoalRecord {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    goal: Option<Box<GoalIndexProjection>>,
    #[serde(default, skip_serializing_if = "is_false")]
    removed: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Goals that still hold the scheduler's attention.
///
/// Scheduling decisions never need a project's whole history. Feature ordering
/// is decided by Goals that still hold their Feature's queue, and priority by
/// Goals still waiting to run; a Goal that reached Review, Done, or
/// Cancelled influences neither. In a mature project those are the overwhelming
/// majority, so an index of the rest is bounded by work in flight rather than by
/// how much work has ever been done.
///
/// This is what decouples scheduler memory from project size. Loading the full
/// projection to schedule meant holding every Goal ever created, which does not
/// survive five million of them.
#[derive(Clone, Debug, Default)]
pub struct ActiveGoalIndex {
    goals: BTreeMap<String, GoalIndexProjection>,
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

    /// Read the index, reconstructing it from Goal records if it is absent.
    ///
    /// Reconstruction streams: each record is projected and then dropped unless
    /// it is still active, so even a first run on a very large project holds
    /// only the live set rather than the whole corpus.
    pub fn load_or_rebuild(refine_dir: &Path) -> RefineResult<Self> {
        if let Some(index) = Self::load(refine_dir)? {
            return Ok(index);
        }
        let index = Self::rebuild(refine_dir)?;
        index.persist(refine_dir)?;
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
        Self::rebuild(refine_dir)?.persist(refine_dir)
    }

    fn load(refine_dir: &Path) -> RefineResult<Option<Self>> {
        let path = Self::path(refine_dir);
        let Ok(file) = File::open(&path) else {
            return Ok(None);
        };
        let mut goals = BTreeMap::new();
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
            match record.goal {
                Some(goal) if !record.removed => {
                    goals.insert(record.id, *goal);
                }
                _ => {
                    goals.remove(&record.id);
                }
            }
        }
        let index = Self { goals };
        if lines > COMPACTION_MINIMUM_LINES && lines > index.goals.len() * COMPACTION_LINE_RATIO {
            index.persist(refine_dir)?;
        }
        Ok(Some(index))
    }

    /// Project every Goal record, keeping only those that still matter.
    pub fn rebuild(refine_dir: &Path) -> RefineResult<Self> {
        let store = FileProjectProjectionStore::new(refine_dir);
        let mut goals = BTreeMap::new();
        for path in FileProjectProjectionStore::collect_goal_record_paths(refine_dir)? {
            let Some(projection) = store.project_goal(&path)? else {
                continue;
            };
            if Self::holds_scheduler_attention(&projection.goal.status) {
                goals.insert(projection.goal.id.clone(), projection.goal);
            }
        }
        Ok(Self { goals })
    }

    /// Rewrite the log so it contains exactly the live set.
    pub fn persist(&self, refine_dir: &Path) -> RefineResult<()> {
        let path = Self::path(refine_dir);
        let mut encoded = String::new();
        for goal in self.goals.values() {
            let record = ActiveGoalRecord {
                id: goal.id.clone(),
                goal: Some(Box::new(goal.clone())),
                removed: false,
            };
            let line = serde_json::to_string(&record).map_err(|error| {
                RefineError::Serialization(format!("failed to encode active Goal record: {error}"))
            })?;
            encoded.push_str(&line);
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
        let store = FileProjectProjectionStore::new(refine_dir);
        let record = match store.project_goal(goal_path)? {
            Some(projection) if Self::holds_scheduler_attention(&projection.goal.status) => {
                ActiveGoalRecord {
                    id: projection.goal.id.clone(),
                    goal: Some(Box::new(projection.goal)),
                    removed: false,
                }
            }
            Some(projection) => ActiveGoalRecord {
                id: projection.goal.id,
                goal: None,
                removed: true,
            },
            None => return Ok(()),
        };
        Self::append(refine_dir, &record)
    }

    /// Record that a Goal no longer exists.
    pub fn forget_goal(refine_dir: &Path, goal_id: &str) -> RefineResult<()> {
        Self::append(
            refine_dir,
            &ActiveGoalRecord {
                id: goal_id.to_string(),
                goal: None,
                removed: true,
            },
        )
    }

    fn append(refine_dir: &Path, record: &ActiveGoalRecord) -> RefineResult<()> {
        let path = Self::path(refine_dir);
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
        // A record can exceed the size the platform appends atomically, and
        // several processes write Goals concurrently, so an interleaved line
        // would corrupt the replay. The lock is held only for one append.
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock active Goal index {}: {error}",
                path.display()
            ))
        })?;
        let write = writeln!(file, "{line}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append active Goal index {}: {error}",
                path.display()
            ))
        });
        let _ = FileExt::unlock(&file);
        write
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
}
