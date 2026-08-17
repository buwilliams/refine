use super::*;

pub(super) const TEST_BASELINE_INTERRUPTION: &str =
    "simulated interruption after recovery baseline publication";

type AfterRecoveryAuthorityHook = Box<dyn FnOnce() + Send + 'static>;
type DuringRecoveryPreviewHook = Box<dyn FnOnce() + Send + 'static>;

fn during_recovery_preview_hooks() -> &'static Mutex<BTreeMap<PathBuf, DuringRecoveryPreviewHook>> {
    static HOOKS: OnceLock<Mutex<BTreeMap<PathBuf, DuringRecoveryPreviewHook>>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

pub(in crate::tools::host::git_sync) fn install_during_recovery_preview_hook(
    target_root: &std::path::Path,
    hook: impl FnOnce() + Send + 'static,
) {
    during_recovery_preview_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(target_root.to_path_buf(), Box::new(hook));
}

pub(super) fn run_during_recovery_preview_hook(target_root: &std::path::Path) {
    let hook = during_recovery_preview_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(target_root);
    if let Some(hook) = hook {
        hook();
    }
}

fn after_recovery_authority_hooks() -> &'static Mutex<BTreeMap<PathBuf, AfterRecoveryAuthorityHook>>
{
    static HOOKS: OnceLock<Mutex<BTreeMap<PathBuf, AfterRecoveryAuthorityHook>>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

pub(in crate::tools::host::git_sync) fn install_after_recovery_authority_hook(
    target_root: &std::path::Path,
    hook: impl FnOnce() + Send + 'static,
) {
    after_recovery_authority_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(target_root.to_path_buf(), Box::new(hook));
}

pub(super) fn run_after_recovery_authority_hook(target_root: &std::path::Path) {
    let hook = after_recovery_authority_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(target_root);
    if let Some(hook) = hook {
        hook();
    }
}

fn after_recovery_baseline_hooks() -> &'static Mutex<BTreeSet<PathBuf>> {
    static HOOKS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

pub(in crate::tools::host::git_sync) fn install_after_recovery_baseline_hook(
    target_root: &std::path::Path,
) {
    after_recovery_baseline_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(target_root.to_path_buf());
}

pub(super) fn run_after_recovery_baseline_hook(target_root: &std::path::Path) -> bool {
    after_recovery_baseline_hooks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(target_root)
}
