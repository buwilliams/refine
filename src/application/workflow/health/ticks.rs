use super::*;
use std::cell::RefCell;
thread_local! { static OBSERVATION: RefCell<Option<SchedulerObservation>> = const { RefCell::new(None) }; }

/// Called only by the workflow scheduling thread, including while attempts are active.
pub(crate) fn scheduler_tick(
    root: &Path,
    target: Option<&Path>,
    active: &BTreeSet<String>,
    completed: bool,
    failure: Option<&str>,
) {
    let token = std::env::var("REFINE_WORKFLOW_INCARNATION").ok();
    #[cfg(test)]
    let token =
        token.or_else(|| OBSERVATION.with(|v| v.borrow().as_ref().map(|o| o.incarnation.clone())));
    let Some(token) = token else {
        return;
    };
    let result = OBSERVATION.with(|state| -> RefineResult<()> {
        let mut state = state.borrow_mut();
        let now = chrono::Utc::now().timestamp_millis();
        if state.is_none() {
            let process = FileProcessSupervisor::new(root)
                .list()?
                .into_iter()
                .find(|p| {
                    p.pid == Some(std::process::id())
                        && is_workflow_worker(p)
                        && workflow_incarnation(p).as_deref() == Some(&token)
                });
            // Tokens are inherited by owned children. Only the actual registered worker
            // publishes ticks; absence leaves independent supervision without readiness proof.
            let Some(process) = process else {
                return Ok(());
            };
            *state = Some(SchedulerObservation {
                runtime_root: root
                    .canonicalize()
                    .map_err(|e| RefineError::Io(e.to_string()))?,
                process_id: process.id,
                pid: std::process::id(),
                os_identity: current_os_identity(std::process::id())?.ok_or_else(|| {
                    RefineError::Degraded("worker OS identity is unavailable".into())
                })?,
                incarnation: token,
                target_root: None,
                node_id: None,
                sequence: 0,
                tick_ms: 0,
                completed_cycle_ms: None,
                active_attempts: BTreeSet::new(),
                failure: None,
                retry_delays: Default::default(),
            });
        }
        let observation = state.as_mut().unwrap();
        if !completed && failure.is_none() && now - observation.tick_ms < 500 {
            return Ok(());
        }
        let target = target
            .map(Path::canonicalize)
            .transpose()
            .map_err(|e| RefineError::Io(e.to_string()))?;
        if target != observation.target_root {
            observation.completed_cycle_ms = None;
            observation.node_id = target
                .as_ref()
                .and_then(|target| {
                    crate::infrastructure::storage::project_layout::refine_dir_for_target_root(
                        target,
                    )
                    .ok()
                })
                .and_then(|dir| {
                    crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
                        dir, root,
                    )
                    .active_node_id()
                    .ok()
                });
        }
        observation.target_root = target;
        observation.sequence = observation.sequence.saturating_add(1);
        observation.tick_ms = now;
        if completed {
            observation.completed_cycle_ms = Some(now);
        }
        observation.active_attempts = active.clone();
        observation.retry_delays.clear();
        if completed || failure.is_some() {
            observation.failure = failure.map(str::to_string);
        }
        observation.write(root)
    });
    if let Err(error) = result {
        eprintln!("refine scheduler observation: {error}");
    }
}

#[cfg(test)]
pub(crate) fn install_test_observation(observation: SchedulerObservation) {
    OBSERVATION.with(|v| *v.borrow_mut() = Some(observation));
}
