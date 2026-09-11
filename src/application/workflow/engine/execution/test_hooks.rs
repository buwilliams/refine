use super::*;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
type Hook = Arc<
    dyn Fn(&WorkflowEngine, &str, &str, WorkflowAttemptAuthority) -> RefineResult<()> + Send + Sync,
>;
static HOOKS: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Hook>>> = OnceLock::new();
static FAILURE_REPORTS: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Vec<serde_json::Value>>>> =
    OnceLock::new();
pub(crate) fn capture_failure(root: &std::path::Path, bytes: &[u8]) {
    FAILURE_REPORTS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(root.into())
        .or_default()
        .push(serde_json::from_slice(bytes).unwrap());
}
pub(crate) fn take_failures(root: &std::path::Path) -> Vec<serde_json::Value> {
    FAILURE_REPORTS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(root)
        .unwrap_or_default()
}
pub(crate) fn install(root: &std::path::Path, hook: Hook) {
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(root.into(), hook);
}
pub(crate) fn remove(root: &std::path::Path) {
    take_failures(root);
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(root);
}
pub(crate) fn run(
    engine: &WorkflowEngine,
    goal: &str,
    stage: &str,
    authority: WorkflowAttemptAuthority,
) -> RefineResult<()> {
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&engine.runtime_root)
        .cloned();
    if let Some(hook) = hook {
        hook(engine, goal, stage, authority)?;
    }
    Ok(())
}
pub(super) fn scheduler(engine: &WorkflowEngine) -> RefineResult<()> {
    run(
        engine,
        "",
        "scheduler",
        WorkflowAttemptAuthority {
            round_idx: 0,
            workflow_revision: 0,
        },
    )
}
