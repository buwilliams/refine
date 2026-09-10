//! Continuously eligible Todo observation, independent of the worker being diagnosed.
use super::*;
use crate::application::projects::projection::ActiveGoalIndex;
use crate::application::workflow::WorkflowEngine;
use crate::application::workflow::engine::policy::SchedulingEligibility;
use crate::infrastructure::process::supervisor::coordination::with_lock_timeout;
use crate::model::workflow::GoalStatus;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AdmissionHealth {
    pub runtime_root: PathBuf,
    pub target_root: PathBuf,
    pub node_id: String,
    pub observer_pid: u32,
    pub observer_os_identity: String,
    pub checked_at_ms: i64,
    pub eligible_since_ms: BTreeMap<String, i64>,
    pub cause: String,
    pub free_capacity: bool,
    pub active_work: bool,
}
impl AdmissionHealth {
    pub fn waiting_count(&self, now: i64) -> usize {
        if !self.free_capacity || self.active_work {
            return 0;
        }
        self.eligible_since_ms
            .values()
            .filter(|at| now - **at >= SILENCE_MS)
            .count()
    }
}

pub fn observe_admission(runtime: &Path, registry: Option<&Path>) -> RefineResult<()> {
    with_lock_timeout(Duration::from_millis(200), || {
        let target = crate::application::projects::registry::FileProjectRegistryService::new(
            registry.unwrap_or(runtime),
            None,
        )
        .load()?
        .active_app;
        let Some(target) = target else {
            return Ok(());
        };
        let target = PathBuf::from(target)
            .canonicalize()
            .map_err(|e| RefineError::Io(e.to_string()))?;
        let now = chrono::Utc::now().timestamp_millis();
        let path = runtime.join("workflow-admission.json");
        let previous: Option<AdmissionHealth> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let snapshot = sample_admission(runtime, &target, previous.as_ref(), now)?;
        crate::infrastructure::process::subprocess::write_json_atomically(
            &path,
            &serde_json::to_vec(&snapshot).unwrap(),
            "workflow admission observation",
        )
    })
}

fn sample_admission(
    runtime: &Path,
    target: &Path,
    previous: Option<&AdmissionHealth>,
    now: i64,
) -> RefineResult<AdmissionHealth> {
    let workflow = WorkflowEngine::with_target_root(runtime, target);
    let policy = workflow.policy()?;
    let refine =
        crate::infrastructure::storage::project_layout::refine_dir_for_target_root(target)?;
    let supervisor = FileProcessSupervisor::new(runtime);
    let paused = supervisor.pause_state()?;
    let events = crate::application::events::FileEventService::new(&refine);
    let items = crate::application::work_items::FileWorkItemService::new(&refine);
    let index = ActiveGoalIndex::load_or_rebuild(&refine)?;
    let eligibility = SchedulingEligibility::new(index.goals());
    let mut active = BTreeSet::new();
    let mut delayed = BTreeSet::new();
    for root in [runtime.to_path_buf(), runtime.join("agents")] {
        let supervisor = FileProcessSupervisor::new(root);
        for process in supervisor.capacity_processes()? {
            if !supervisor.group_pending(&process)? {
                continue;
            }
            let details: Value = process
                .details
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            if let Some(goal) = details["goal_id"].as_str() {
                active.insert(goal.to_string());
            }
        }
    }
    // The scheduler snapshot also counts preparation and settlement without a child process.
    for worker in supervisor.list()?.into_iter().filter(is_workflow_worker) {
        if !supervisor.owned_process_is_alive(&worker)? {
            continue;
        }
        if let Some(token) = workflow_incarnation(&worker)
            && let Ok(snapshot) = SchedulerObservation::read(runtime, &token)
            && snapshot.process_id == worker.id
            && Some(snapshot.pid) == worker.pid
            && snapshot.node_id.as_deref() == Some(policy.active_node_id.as_str())
            && snapshot.runtime_root
                == runtime
                    .canonicalize()
                    .map_err(|e| RefineError::Io(e.to_string()))?
            && current_os_identity(snapshot.pid)?.as_ref() == Some(&snapshot.os_identity)
            && snapshot.target_root.as_deref() == Some(target)
        {
            active.extend(snapshot.active_attempts);
            delayed.extend(
                snapshot
                    .retry_delays
                    .into_iter()
                    .filter(|(_, at)| *at > now)
                    .map(|(id, _)| id),
            );
        }
    }
    let load = workflow.observed_execution_load()?;
    let free_capacity = load.available(
        &policy,
        &policy.active_node_id,
        &policy.provider,
        &policy.target_app_id,
    );
    let mut cause = if paused.workflow_paused {
        "paused".to_string()
    } else if paused.disabled_background_workers.contains("workflow") {
        "disabled".into()
    } else {
        String::new()
    };
    let mut ids = Vec::new();
    if cause.is_empty() {
        for goal in index.goals().filter(|g| {
            g.status == GoalStatus::Todo
                && g.round_count > 0
                && crate::application::fleet::nodes::node_ids_match(
                    g.node_id.as_deref().unwrap_or("default"),
                    &policy.active_node_id,
                )
        }) {
            if let Some(missing) = events.missing_workflow_requirement(
                &items.show_goal_detail(&goal.id)?,
                &policy.active_node_id,
            )? {
                cause = missing;
                continue;
            }
            if delayed.contains(&goal.id) {
                cause = "retry delay".into();
                continue;
            }
            if eligibility.feature_eligible(&goal.id) && eligibility.priority_eligible(goal) {
                ids.push(goal.id.clone());
            } else {
                cause = "scope or ordering blocks".into();
            }
        }
    }
    let previous = previous.filter(|p| {
        p.target_root == target
            && p.node_id == policy.active_node_id
            && p.observer_pid == std::process::id()
            && (0..3_000).contains(&(now - p.checked_at_ms))
    });
    let mut eligible_since_ms = ids
        .into_iter()
        .map(|id| {
            let since = previous
                .and_then(|p| p.eligible_since_ms.get(&id))
                .copied()
                .unwrap_or(now);
            (id, since)
        })
        .collect::<BTreeMap<_, _>>();
    if !free_capacity || !active.is_empty() {
        eligible_since_ms.clear();
    }
    if !active.is_empty() {
        cause = "live attempts or interactive Goal sessions".into();
    } else if !free_capacity {
        cause = "capacity is occupied".into();
    } else if !eligible_since_ms.is_empty() {
        cause = "eligible Todo work awaiting admission".into();
    } else if cause.is_empty() {
        cause = "no eligible Todo work".into();
    }
    Ok(AdmissionHealth {
        runtime_root: runtime
            .canonicalize()
            .map_err(|e| RefineError::Io(e.to_string()))?,
        observer_os_identity: current_os_identity(std::process::id())?.ok_or_else(|| {
            RefineError::Degraded("admission observer identity unavailable".into())
        })?,
        target_root: target.into(),
        node_id: policy.active_node_id,
        observer_pid: std::process::id(),
        checked_at_ms: now,
        eligible_since_ms,
        cause,
        free_capacity,
        active_work: !active.is_empty(),
    })
}

pub(super) fn read_admission(
    root: &Path,
    target: Option<&Path>,
    now: i64,
) -> Option<AdmissionHealth> {
    let bytes = std::fs::read(root.join("workflow-admission.json")).ok()?;
    let snapshot: AdmissionHealth = serde_json::from_slice(&bytes).ok()?;
    (root.canonicalize().ok().as_ref() == Some(&snapshot.runtime_root)
        && current_os_identity(snapshot.observer_pid)
            .ok()
            .flatten()
            .as_ref()
            == Some(&snapshot.observer_os_identity)
        && target.and_then(|p| p.canonicalize().ok()).as_ref() == Some(&snapshot.target_root)
        && super::observed_node(root, target).ok().flatten().as_ref() == Some(&snapshot.node_id)
        && (0..3_000).contains(&(now - snapshot.checked_at_ms)))
    .then_some(snapshot)
}

#[cfg(test)]
mod tests;
