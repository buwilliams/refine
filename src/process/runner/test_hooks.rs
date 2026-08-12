use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BackgroundWorkerBoundary {
    EnsureLaunch,
    BeforeOperation,
    AfterOperation,
}

type BackgroundWorkerHook = Arc<dyn Fn(&str, BackgroundWorkerBoundary) + Send + Sync>;

fn hook_slot() -> &'static Mutex<Option<BackgroundWorkerHook>> {
    static SLOT: OnceLock<Mutex<Option<BackgroundWorkerHook>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub(super) struct BackgroundWorkerHookGuard;

impl Drop for BackgroundWorkerHookGuard {
    fn drop(&mut self) {
        *hook_slot().lock().unwrap() = None;
    }
}

pub(super) fn install_background_worker_hook(
    hook: impl Fn(&str, BackgroundWorkerBoundary) + Send + Sync + 'static,
) -> BackgroundWorkerHookGuard {
    *hook_slot().lock().unwrap() = Some(Arc::new(hook));
    BackgroundWorkerHookGuard
}

pub(super) fn run_background_worker_hook(worker_kind: &str, boundary: BackgroundWorkerBoundary) {
    let hook = hook_slot().lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(worker_kind, boundary);
    }
}
