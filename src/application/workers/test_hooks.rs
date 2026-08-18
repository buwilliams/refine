use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BackgroundWorkerBoundary {
    EnsureLaunch,
    BeforeOperation,
    AfterOperation,
}

/// The boundary observer, called with the runtime root the operation belongs
/// to. Tests share one process, so an installed hook sees every test's
/// background operations; the runtime root is what lets an observer keep only
/// its own.
type BackgroundWorkerHook = Arc<dyn Fn(&Path, &str, BackgroundWorkerBoundary) + Send + Sync>;

fn hook_slot() -> &'static Mutex<Option<BackgroundWorkerHook>> {
    static SLOT: OnceLock<Mutex<Option<BackgroundWorkerHook>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// One installer at a time: the hook slot is process-global, so two tests
/// installing concurrently would each observe the other's worker boundaries
/// (and lose their own). Held for the guard's lifetime, so a second installer
/// waits rather than clobbering.
fn installer_exclusion() -> &'static Mutex<()> {
    static EXCLUSION: OnceLock<Mutex<()>> = OnceLock::new();
    EXCLUSION.get_or_init(|| Mutex::new(()))
}

pub(super) struct BackgroundWorkerHookGuard {
    _exclusion: MutexGuard<'static, ()>,
}

impl Drop for BackgroundWorkerHookGuard {
    fn drop(&mut self) {
        *hook_slot().lock().unwrap() = None;
    }
}

pub(super) fn install_background_worker_hook(
    hook: impl Fn(&Path, &str, BackgroundWorkerBoundary) + Send + Sync + 'static,
) -> BackgroundWorkerHookGuard {
    // A test that panicked while installed poisoned nothing but the lock
    // itself; the next installer still gets an exclusive, empty slot.
    let exclusion = installer_exclusion()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *hook_slot().lock().unwrap() = Some(Arc::new(hook));
    BackgroundWorkerHookGuard {
        _exclusion: exclusion,
    }
}

pub(super) fn run_background_worker_hook(
    runtime_root: &Path,
    worker_kind: &str,
    boundary: BackgroundWorkerBoundary,
) {
    let hook = hook_slot().lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(runtime_root, worker_kind, boundary);
    }
}
