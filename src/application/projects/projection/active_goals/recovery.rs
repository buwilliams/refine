//! Recover the gap between a durable Goal decision and its derived scheduler index.
use super::*;
use sha2::{Digest, Sha256};

const PENDING_DIRECTORY: &str = "runtime/active-goals-pending";

#[derive(Deserialize, Serialize)]
struct PendingProjection {
    path: PathBuf,
    fingerprint: Option<String>,
}

impl ActiveGoalIndex {
    fn pending_path(refine_dir: &Path, goal_path: &Path) -> PathBuf {
        let key = record_lock_key(goal_path);
        let encoded = key
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        refine_dir
            .join(PENDING_DIRECTORY)
            .join(format!("{encoded}.json"))
    }

    /// Must precede the source replacement while its existing record lock is held.
    /// If the writer exits at either side of the replacement, the next reader
    /// reprojects that one source. No corpus-wide freshness scan is necessary.
    pub(crate) fn prepare_goal_write(
        refine_dir: &Path,
        goal_path: &Path,
        source: Option<&[u8]>,
    ) -> RefineResult<()> {
        let relative = goal_path
            .strip_prefix(refine_dir)
            .map_err(|error| RefineError::InvalidInput(error.to_string()))?;
        let bytes = serde_json::to_vec(&PendingProjection {
            path: relative.into(),
            fingerprint: source.map(|bytes| format!("{:x}", Sha256::digest(bytes))),
        })
        .map_err(|error| RefineError::Serialization(error.to_string()))?;
        Self::with_index_lock(refine_dir, || {
            replace_file_durably(&Self::pending_path(refine_dir, goal_path), &bytes)
        })
    }

    /// Writers call this under their record lock. Replay needs only the index
    /// lock: the pinned source bytes prevent it from clearing a marker before
    /// replacement, without acquiring another Goal's record lock while reading.
    pub(crate) fn complete_goal_write(refine_dir: &Path, goal_path: &Path) -> RefineResult<()> {
        Self::with_index_lock(refine_dir, || {
            let marker = Self::pending_path(refine_dir, goal_path);
            let pending: PendingProjection = match fs::read(&marker) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|error| RefineError::Serialization(error.to_string()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Self::record_goal_locked(refine_dir, goal_path);
                }
                Err(error) => return Err(RefineError::Io(error.to_string())),
            };
            let source = match fs::read(goal_path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(RefineError::Io(error.to_string())),
            };
            // The source content is observed before projection. A writer may be
            // completing that source replacement now, but cannot publish another
            // pending marker while this index lock is held.
            Self::record_goal_locked(refine_dir, goal_path)?;
            if source
                .as_ref()
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                != pending.fingerprint
            {
                // The source writer has not completed its replacement yet. Its
                // durable marker remains visible through that interruption window.
                return Ok(());
            }
            if let Some(bytes) = source {
                let current: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| RefineError::Serialization(error.to_string()))?;
                crate::application::events::transitions::repair_goal_dispatches(
                    refine_dir, goal_path, &current,
                )?;
            }
            match fs::remove_file(marker) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(RefineError::Io(format!(
                    "failed to clear pending Goal discovery: {error}"
                ))),
            }
        })
    }

    pub(super) fn reconcile_pending(refine_dir: &Path) -> RefineResult<()> {
        let entries = match fs::read_dir(refine_dir.join(PENDING_DIRECTORY)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read pending Goal discovery: {error}"
                )));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| RefineError::Io(error.to_string()))?;
            let marker = entry.path();
            if marker.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let repair = (|| -> RefineResult<()> {
                let bytes = match fs::read(&marker) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(RefineError::Io(error.to_string())),
                };
                let pending: PendingProjection = match serde_json::from_slice(&bytes) {
                    Ok(pending) => pending,
                    Err(error) => {
                        // Its filename still identifies the source even when
                        // this disposable marker's contents are damaged. Keep
                        // retrying that source until a subsequent write replaces
                        // the marker; do not consume a concurrent writer's work.
                        Self::reconcile_damaged_marker(refine_dir, &marker)?;
                        return Err(RefineError::Serialization(error.to_string()));
                    }
                };
                let relative = pending.path;
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|component| !matches!(component, std::path::Component::Normal(_)))
                {
                    return Err(RefineError::InvalidInput(
                        "pending Goal discovery has an invalid source path".into(),
                    ));
                }
                let path = refine_dir.join(relative);
                Self::complete_goal_write(refine_dir, &path)
            })();
            if let Err(error) = repair {
                // One unavailable source must not silence its healthy siblings.
                // Retain the marker so recovery is retried on the next pass.
                eprintln!(
                    "refine Goal discovery recovery {}: {error}",
                    marker.display()
                );
            }
        }
        Ok(())
    }

    fn reconcile_damaged_marker(refine_dir: &Path, marker: &Path) -> RefineResult<()> {
        let encoded = marker
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let bytes = encoded
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            })
            .collect::<Option<Vec<_>>>();
        let id = bytes
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|id| {
                id.len() >= 3
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
            .ok_or_else(|| {
                RefineError::InvalidInput(
                    "damaged discovery marker has no recoverable Goal identity".into(),
                )
            })?;
        let sharded = refine_dir
            .join("goals")
            .join(&id[..2])
            .join(&id[2..])
            .join("goal.json");
        let legacy = refine_dir.join("goals").join(&id).join("goal.json");
        let path = if !sharded.exists() && legacy.exists() {
            legacy
        } else {
            sharded
        };
        Self::record_goal(refine_dir, &path)
    }

    pub(super) fn projection_error(
        refine_dir: &Path,
        path: &Path,
        error: Option<RefineError>,
    ) -> GoalProjectionError {
        GoalProjectionError {
            path: path.strip_prefix(refine_dir).unwrap_or(path).to_path_buf(),
            message: format!(
                "Goal source {} needs recovery: {}",
                path.display(),
                error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "record must be an object with a Goal id".into())
            ),
        }
    }

    pub(super) fn with_index_lock<T>(
        refine_dir: &Path,
        action: impl FnOnce() -> RefineResult<T>,
    ) -> RefineResult<T> {
        let path = refine_dir.join("runtime/.active-goals.lock");
        fs::create_dir_all(path.parent().unwrap())
            .map_err(|error| RefineError::Io(error.to_string()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| RefineError::Io(error.to_string()))?;
        // Lock a stable inode: compaction replaces the index file itself. All
        // append, load, and rebuild paths use this lock, preventing a concurrent
        // append from being lost under a compacted or rebuilt snapshot.
        file.lock_exclusive()
            .map_err(|error| RefineError::Io(error.to_string()))?;
        let result = action();
        let _ = FileExt::unlock(&file);
        result
    }
}
