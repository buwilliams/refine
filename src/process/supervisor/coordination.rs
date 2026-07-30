use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use uuid::Uuid;

use crate::process::supervisor::errors::{RefineError, RefineResult};

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

thread_local! {
    static HELD_COORDINATION_LOCKS: RefCell<BTreeMap<PathBuf, usize>> =
        const { RefCell::new(BTreeMap::new()) };
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
