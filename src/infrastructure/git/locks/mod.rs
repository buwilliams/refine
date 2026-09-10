use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fs2::FileExt;

use crate::error::{RefineError, RefineResult};

/// How long to wait for the repository lock before reporting contention.
///
/// Longer than the Git stall budget on purpose: a legitimately slow operation
/// that keeps reporting progress must be allowed to finish rather than have its
/// waiters give up underneath it, while a wedged one is stopped by its own
/// budget and releases the lock well inside this window.
const REPOSITORY_LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(600);
const REPOSITORY_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
static REPOSITORY_GIT_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

thread_local! { static LOCK_BUDGET: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) }; }
pub fn with_repository_lock_timeout<T>(timeout: Duration, operation: impl FnOnce() -> T) -> T {
    struct Reset(Option<Duration>);
    impl Drop for Reset {
        fn drop(&mut self) {
            LOCK_BUDGET.with(|v| v.set(self.0));
        }
    }
    let _reset = Reset(LOCK_BUDGET.with(|v| v.replace(Some(timeout))));
    operation()
}
fn lock_budget() -> Duration {
    LOCK_BUDGET
        .with(|v| v.get())
        .unwrap_or(REPOSITORY_LOCK_ACQUIRE_TIMEOUT)
}

pub fn with_repository_git_lock<T>(
    target_root: &std::path::Path,
    action: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    let lock = repository_git_lock(target_root)?;
    let deadline = Instant::now() + lock_budget();
    let _guard = loop {
        match lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(RefineError::Conflict(
                    "Repository Git lock was poisoned".into(),
                ));
            }
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() >= deadline => {
                return Err(RefineError::Degraded(
                    "Repository Git lock is busy; retry later".into(),
                ));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let _file_guard = RepositoryFileLock::acquire(target_root)?;
    action()
}

pub(crate) fn repository_git_lock(target_root: &std::path::Path) -> RefineResult<Arc<Mutex<()>>> {
    let key = target_root
        .canonicalize()
        .unwrap_or_else(|_| target_root.to_path_buf());
    {
        let mut locks = REPOSITORY_GIT_LOCKS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| RefineError::Conflict("Git lock registry was poisoned".to_string()))?;
        Ok(Arc::clone(
            locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }
}

pub(crate) struct RepositoryFileLock {
    file: Option<File>,
}

impl RepositoryFileLock {
    /// Take the repository lock, giving up at the deadline.
    ///
    /// Acquisition used to block forever, so a holder that wedged stopped every
    /// other repository operation permanently and looked exactly like an idle
    /// system. Giving up turns that into a reported contention that the caller's
    /// own cadence retries.
    fn acquire(target_root: &std::path::Path) -> RefineResult<Self> {
        let Some(file) = repository_lock_file(target_root)? else {
            return Ok(Self { file: None });
        };
        let deadline = Instant::now() + lock_budget();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file: Some(file) }),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock repository {}: {error}",
                        target_root.display()
                    )));
                }
            }
            if Instant::now() >= deadline {
                return Err(RefineError::Degraded(format!(
                    "repository {} stayed locked by another operation for {}ms",
                    target_root.display(),
                    lock_budget().as_millis()
                )));
            }
            std::thread::sleep(REPOSITORY_LOCK_POLL_INTERVAL);
        }
    }

    pub(crate) fn try_acquire(target_root: &std::path::Path) -> RefineResult<Option<Self>> {
        let Some(file) = repository_lock_file(target_root)? else {
            return Ok(Some(Self { file: None }));
        };
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file: Some(file) })),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to lock repository {}: {error}",
                target_root.display()
            ))),
        }
    }
}

impl Drop for RepositoryFileLock {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = FileExt::unlock(file);
        }
    }
}

fn repository_lock_file(target_root: &std::path::Path) -> RefineResult<Option<File>> {
    let common_dir =
        match crate::infrastructure::storage::project_layout::git_common_dir(target_root) {
            Ok(dir) => dir,
            Err(RefineError::Io(error)) => return Err(RefineError::Io(error)),
            Err(_) => return Ok(None),
        };
    let path = common_dir.join("refine-repository.lock");
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map(Some)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open repository lock {}: {error}",
                path.display()
            ))
        })
}
