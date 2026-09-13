//! Retire execution after a durable workflow decision, independently of scheduling.
use super::*;
use crate::application::events::FileEventService;
use crate::application::work_items::FileWorkItemService;

pub(super) fn maintain_execution(
    supervisor: &FileProcessSupervisor,
    group: &OwnedGroup,
) -> RefineResult<bool> {
    if !superseded(&group.process)? {
        return Ok(false);
    }
    // Read the Goal before process coordination to avoid inverting launch locks.
    crate::infrastructure::process::supervisor::coordination::with_lock_timeout(
        Duration::from_millis(200),
        || {
            let observed = supervisor.observe_owned_group(group)?;
            if observed.confirmed_exit {
                return Ok(false);
            }
            let stopped = supervisor.stop_owned_group(&observed, Duration::from_millis(200))?;
            if !stopped.confirmed_exit {
                return Err(RefineError::Degraded(format!(
                    "superseded process {} exit is unconfirmed; cleanup will retry",
                    group.process.id,
                )));
            }
            Ok(true)
        },
    )
}

pub(super) fn maintain_ungrouped_execution(
    supervisor: &FileProcessSupervisor,
    process: &ManagedProcess,
) -> RefineResult<bool> {
    if !superseded(process)? || !supervisor.owned_process_is_alive(process)? {
        return Ok(false);
    }
    // Legacy registrations without retained groups still have exact registration-
    // time PID evidence. Never signal by a reusable registry name or PID alone.
    supervisor
        .terminate_owned_and_confirm_exit(process, "terminate", Duration::from_millis(200))
        .or_else(|_| {
            supervisor.terminate_owned_and_confirm_exit(process, "kill", Duration::from_millis(200))
        })?;
    Ok(true)
}

fn superseded(process: &ManagedProcess) -> RefineResult<bool> {
    let metadata = process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .unwrap_or(Value::Null);
    // A started Git publication finishes its own atomic operation. Its caller
    // must recheck the decision before starting any subsequent publication.
    if metadata["side_effect_committed"] == true {
        return Ok(false);
    }
    let (Some(goal_id), Some(target)) = (
        metadata["goal_id"].as_str(),
        metadata["target_app_id"].as_str(),
    ) else {
        return Ok(false);
    };
    let refine_dir = refine_dir_for_target_root(Path::new(target))?;
    let goal = FileWorkItemService::new(&refine_dir).show_goal_detail(goal_id)?;
    let Some(decision) = goal["workflow_events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|event| event["supersedes_execution"] == true)
        .max_by_key(|event| event["generation"].as_u64().unwrap_or(0))
    else {
        return Ok(false);
    };
    let mut generation = metadata["workflow_step_generation"].as_u64();
    let mut revision = metadata["workflow_revision"].as_u64();
    if generation.is_none()
        && revision.is_none()
        && let Some(id) = metadata["event_invocation_id"].as_str()
    {
        // Pre-upgrade Skills did not all publish occurrence metadata. Their
        // durable invocation retains the pinned snapshot, including the caller.
        let invocation = FileEventService::new(&refine_dir).invocation(id)?;
        generation = invocation.context.data["goal"]["event_generation"].as_u64();
        revision = invocation
            .context
            .workflow_revision
            .or_else(|| invocation.context.data["goal"]["workflow_revision"].as_u64());
    }
    let superseded = match (generation, decision["generation"].as_u64()) {
        (Some(launched), Some(selected)) => launched < selected,
        _ => match (revision, decision["workflow_revision"].as_u64()) {
            (Some(launched), Some(selected)) => launched < selected,
            _ => {
                return Err(RefineError::Degraded(format!(
                    "Goal {goal_id} process {} has no verifiable workflow occurrence for downstream cleanup",
                    process.id,
                )));
            }
        },
    };
    // A newer decision may advance the cutoff, but cannot reauthorize this old
    // process. New execution has a later registration and launch occurrence.
    Ok(superseded)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
