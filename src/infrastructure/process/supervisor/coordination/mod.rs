use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use uuid::Uuid;

use crate::error::{RefineError, RefineResult};

pub const WORKFLOW_COORDINATION_LOCK: &str = ".workflow-coordination.lock";
/// How long to wait for the coordination lock before giving up.
///
/// Acquisition used to block forever, so one holder that wedged — a Git command
/// with no deadline, a process killed between taking the lock and releasing it —
/// stopped every other caller permanently, with no diagnostic distinguishing
/// that from ordinary idleness. A caller that gives up reports a contended lock
/// and can be retried by its own cadence; a caller that blocks forever cannot.
const COORDINATION_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(60);
const COORDINATION_ACQUIRE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Take an exclusive lock, giving up at `timeout`.
///
/// `fs2` offers only a blocking or an immediate attempt, so a bounded wait is
/// polled. The interval is short enough that ordinary contention is
/// indistinguishable from a blocking acquire.
fn lock_exclusive_before(file: &File, path: &Path, timeout: Duration) -> RefineResult<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to acquire workflow coordination lock {}: {error}",
                    path.display()
                )));
            }
        }
        if Instant::now() >= deadline {
            return Err(RefineError::Degraded(format!(
                "workflow coordination lock {} was held by another operation for {}s",
                path.display(),
                timeout.as_secs()
            )));
        }
        std::thread::sleep(COORDINATION_ACQUIRE_POLL_INTERVAL);
    }
}

/// Where record locks live. Under `runtime/` because a lock describes this
/// host's in-flight work, not project state, and must never synchronize.
const RECORD_LOCK_DIR: &str = "runtime/record-locks";
/// How many locks the record space is divided across.
///
/// One lock per record would mean an inode per Goal, which does not survive a
/// project of millions; a single lock is what this replaces. Striping keeps the
/// file count fixed while letting unrelated records proceed in parallel —
/// collisions only cost the serialization that used to apply to everything.
const RECORD_LOCK_STRIPES: u64 = 64;

thread_local! {
    static HELD_COORDINATION_LOCKS: RefCell<BTreeMap<PathBuf, usize>> =
        const { RefCell::new(BTreeMap::new()) };
    static HELD_RECORD_LOCKS: RefCell<BTreeMap<PathBuf, usize>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// The identity a record is locked under.
///
/// A Goal's own identifier, so that locking a Goal and then writing its record
/// resolve to the same stripe and nest re-entrantly. Deriving one from the path
/// and the other from the id would put a mutation behind two different locks
/// and deadlock it against itself.
pub fn record_lock_key(path: &Path) -> String {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        if matches!(component.as_str(), "goals" | "features") {
            let identity = components
                .get(index + 1..components.len().saturating_sub(1))
                .unwrap_or_default()
                .concat();
            if !identity.is_empty() {
                return identity;
            }
        }
    }
    path.to_string_lossy().into_owned()
}

/// Serialize writers of one record without serializing writers of all records.
///
/// Every Goal mutation used to take a single lock covering the whole target
/// application, so two unrelated Goals could not be written at the same time
/// however much capacity the host had. What that lock actually has to protect
/// is one record's read-compare-write against another writer of the *same*
/// record; the durable revision check inside that window is what makes the
/// write safe, and it only needs to be atomic per record.
///
/// Re-entrant, because a mutation locks a Goal and then writes its record, and
/// `flock` on a second descriptor for the same file would otherwise block
/// against the caller's own lock.
pub fn acquire_record_lock(root: &Path, key: &str) -> RefineResult<RecordLease> {
    let root = coordination_lock_root(root);
    let mut stripe = 0xcbf29ce484222325u64;
    for byte in key.as_bytes() {
        stripe ^= u64::from(*byte);
        stripe = stripe.wrapping_mul(0x100000001b3);
    }
    let directory = root.join(RECORD_LOCK_DIR);
    let path = directory.join(format!("{}.lock", stripe % RECORD_LOCK_STRIPES));

    let nested = HELD_RECORD_LOCKS.with(|locks| {
        let mut locks = locks.borrow_mut();
        let Some(depth) = locks.get_mut(&path) else {
            return false;
        };
        *depth += 1;
        true
    });
    if nested {
        return Ok(RecordLease {
            _depth: RecordLockDepth { path },
            _guard: None,
        });
    }
    fs::create_dir_all(&directory).map_err(|error| {
        RefineError::Io(format!(
            "failed to create record lock directory {}: {error}",
            directory.display()
        ))
    })?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open record lock {}: {error}",
                path.display()
            ))
        })?;
    lock_exclusive_before(&file, &path, COORDINATION_ACQUIRE_TIMEOUT)?;
    HELD_RECORD_LOCKS.with(|locks| {
        locks.borrow_mut().insert(path.clone(), 1);
    });
    Ok(RecordLease {
        _depth: RecordLockDepth { path: path.clone() },
        _guard: Some(WorkflowCoordinationGuard { file, path }),
    })
}

pub fn with_record_lock<T>(
    root: &Path,
    key: &str,
    action: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    let _lease = acquire_record_lock(root, key)?;
    action()
}

pub struct RecordLease {
    _depth: RecordLockDepth,
    _guard: Option<WorkflowCoordinationGuard>,
}

struct RecordLockDepth {
    path: PathBuf,
}

impl Drop for RecordLockDepth {
    fn drop(&mut self) {
        HELD_RECORD_LOCKS.with(|locks| {
            let mut locks = locks.borrow_mut();
            let remove = match locks.get_mut(&self.path) {
                Some(depth) if *depth > 1 => {
                    *depth -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if remove {
                locks.remove(&self.path);
            }
        });
    }
}

pub fn with_workflow_coordination<T>(
    root: &Path,
    action: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    let _lease = acquire_workflow_coordination(root)?;
    action()
}

pub fn acquire_workflow_coordination(root: &Path) -> RefineResult<WorkflowCoordinationLease> {
    let root = coordination_lock_root(root);
    let nested = HELD_COORDINATION_LOCKS.with(|locks| {
        let mut locks = locks.borrow_mut();
        let Some(depth) = locks.get_mut(&root) else {
            return false;
        };
        *depth += 1;
        true
    });
    if nested {
        return Ok(WorkflowCoordinationLease {
            _depth: WorkflowCoordinationDepth { root },
            _guard: None,
        });
    }
    fs::create_dir_all(&root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create workflow coordination directory {}: {error}",
            root.display()
        ))
    })?;
    let path = root.join(WORKFLOW_COORDINATION_LOCK);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open workflow coordination lock {}: {error}",
                path.display()
            ))
        })?;
    lock_exclusive_before(&file, &path, COORDINATION_ACQUIRE_TIMEOUT)?;
    HELD_COORDINATION_LOCKS.with(|locks| {
        locks.borrow_mut().insert(root.clone(), 1);
    });
    Ok(WorkflowCoordinationLease {
        _depth: WorkflowCoordinationDepth { root },
        _guard: Some(WorkflowCoordinationGuard { file, path }),
    })
}

pub fn replace_file_durably(path: &Path, bytes: &[u8]) -> RefineResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!("durable record {} has no parent", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to create durable record directory {}: {error}",
            parent.display()
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("record");
    let temp = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> RefineResult<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to create durable temporary record {}: {error}",
                    temp.display()
                ))
            })?;
        file.write_all(bytes).map_err(|error| {
            RefineError::Io(format!(
                "failed to write durable temporary record {}: {error}",
                temp.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            RefineError::Io(format!(
                "failed to sync durable temporary record {}: {error}",
                temp.display()
            ))
        })?;
        fs::rename(&temp, path).map_err(|error| {
            RefineError::Io(format!(
                "failed to publish durable record {}: {error}",
                path.display()
            ))
        })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to sync durable record directory {}: {error}",
                    parent.display()
                ))
            })
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

fn coordination_lock_root(root: &Path) -> PathBuf {
    match root.file_name().and_then(|name| name.to_str()) {
        Some("refine-live-state" | ".refine") => root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| root.to_path_buf()),
        _ => root.to_path_buf(),
    }
}

struct WorkflowCoordinationGuard {
    file: File,
    path: PathBuf,
}

pub struct WorkflowCoordinationLease {
    _depth: WorkflowCoordinationDepth,
    _guard: Option<WorkflowCoordinationGuard>,
}

struct WorkflowCoordinationDepth {
    root: PathBuf,
}

impl Drop for WorkflowCoordinationDepth {
    fn drop(&mut self) {
        HELD_COORDINATION_LOCKS.with(|locks| {
            let mut locks = locks.borrow_mut();
            let remove = match locks.get_mut(&self.root) {
                Some(depth) if *depth > 1 => {
                    *depth -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if remove {
                locks.remove(&self.root);
            }
        });
    }
}

impl Drop for WorkflowCoordinationGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            eprintln!(
                "refine workflow coordination lock {} could not be released: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Acquisition used to block forever, so one holder that wedged stopped every
    // other caller permanently and was indistinguishable from an idle system.
    #[test]
    fn a_contended_lock_is_reported_rather_than_waited_on_forever() {
        let root = unique_temp_dir("coordination-contended");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(WORKFLOW_COORDINATION_LOCK);
        let holder = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        holder.lock_exclusive().unwrap();

        let waiter = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let started = Instant::now();
        let error = lock_exclusive_before(&waiter, &path, Duration::from_millis(200))
            .expect_err("a held lock must not be acquired");

        assert!(
            matches!(error, RefineError::Degraded(_)),
            "contention is a degraded condition, not an I/O failure: {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waiting did not respect the deadline"
        );

        FileExt::unlock(&holder).unwrap();
        // Once released the lock is ordinary again, so contention never leaves
        // the caller permanently poisoned.
        lock_exclusive_before(&waiter, &path, Duration::from_secs(1)).unwrap();
        FileExt::unlock(&waiter).unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
