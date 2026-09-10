#[cfg(test)]
use super::*;

#[cfg(test)]
type AfterProcessEnumerationHook = Box<dyn FnOnce() + Send + 'static>;

#[cfg(test)]
static AFTER_PROCESS_ENUMERATION_HOOKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, AfterProcessEnumerationHook>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn install_after_process_enumeration_hook(
    runtime_root: &Path,
    hook: impl FnOnce() + Send + 'static,
) {
    AFTER_PROCESS_ENUMERATION_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(runtime_root.to_path_buf(), Box::new(hook));
}

#[cfg(test)]
pub(super) fn run_after_process_enumeration_hook(runtime_root: &Path) {
    let hook = AFTER_PROCESS_ENUMERATION_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(runtime_root);
    if let Some(hook) = hook {
        hook();
    }
}

pub(super) fn tail_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        value.to_string()
    } else {
        value.chars().skip(count - max_chars).collect()
    }
}

#[cfg(test)]
type CaptureHook = Box<dyn FnOnce(&ManagedProcess) + Send + 'static>;
#[cfg(test)]
static CAPTURE_HOOKS: std::sync::OnceLock<
    Mutex<std::collections::BTreeMap<(PathBuf, String), CaptureHook>>,
> = std::sync::OnceLock::new();
#[cfg(test)]
pub(super) fn install_capture_hook(
    root: &Path,
    stage: &str,
    hook: impl FnOnce(&ManagedProcess) + Send + 'static,
) {
    CAPTURE_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert((root.into(), stage.into()), Box::new(hook));
}
#[cfg(test)]
pub(super) fn run_capture_hook(root: &Path, stage: &str, process: &ManagedProcess) {
    let hook = CAPTURE_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(root.into(), stage.into()));
    if let Some(hook) = hook {
        hook(process);
    }
}
