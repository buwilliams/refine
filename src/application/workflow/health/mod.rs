//! Shared Application assessment, evaluated afresh by every diagnostic surface and supervision.
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::subprocess::scheduler_observation::{
    SchedulerObservation, current_os_identity, is_workflow_worker, workflow_incarnation,
};
use crate::infrastructure::process::subprocess::{FileProcessSupervisor, ManagedProcess};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
pub mod admission;
mod ownership;
mod ticks;
#[cfg(test)]
pub(crate) use ticks::install_test_observation;
pub(crate) use ticks::scheduler_tick;
pub const SILENCE_MS: i64 = 30_000;
pub const STARTUP_MS: i64 = 30_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkflowHealth {
    pub healthy: bool,
    pub waiting_todo_count: usize,
    pub state: String,
    pub reason: String,
    pub remedy: String,
    pub observation: Option<SchedulerObservation>,
    pub admission: Option<admission::AdmissionHealth>,
}
impl WorkflowHealth {
    pub fn unavailable(state: &str, reason: impl Into<String>) -> Self {
        Self {
            healthy: false,
            waiting_todo_count: 0,
            state: state.into(),
            reason: reason.into(),
            remedy: "refine system status; refine system doctor".into(),
            observation: None,
            admission: None,
        }
    }
}

pub(super) fn observed_node(root: &Path, target: Option<&Path>) -> RefineResult<Option<String>> {
    target
        .map(|target| {
            let refine_dir =
                crate::infrastructure::storage::project_layout::refine_dir_for_target_root(target)?;
            crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
                refine_dir, root,
            )
            .active_node_id()
        })
        .transpose()
}

pub fn assess_worker(
    root: &Path,
    process: &ManagedProcess,
    target: Option<&Path>,
) -> WorkflowHealth {
    assess_worker_with_clock(root, process, target, || {
        chrono::Utc::now().timestamp_millis()
    })
}

pub(crate) fn assess_worker_with_clock(
    root: &Path,
    process: &ManagedProcess,
    target: Option<&Path>,
    clock: impl FnOnce() -> i64,
) -> WorkflowHealth {
    let incarnation = workflow_incarnation(process);
    let snapshot = incarnation
        .as_deref()
        .ok_or_else(|| RefineError::Degraded("worker incarnation is unavailable".into()))
        .and_then(|token| SchedulerObservation::read(root, token));
    // Read the observation before sampling time: the scheduling thread can publish
    // a newer tick during the read. A time captured by the caller can make that
    // healthy tick look future-dated and trigger termination of the worker.
    let now = clock();
    let started = process.started_at.parse::<i64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc3339(&process.started_at)
            .ok()
            .map(|d| d.timestamp_millis())
    });
    let starting = started.is_some_and(|t| (0..STARTUP_MS).contains(&(now - t)));
    let mut health = match snapshot {
        Ok(snapshot) => {
            let runtime_matches = root.canonicalize().ok().as_ref() == Some(&snapshot.runtime_root);
            let target_matches = target.map(|p| p.canonicalize()).transpose().ok()
                == Some(snapshot.target_root.clone());
            let identity_matches = snapshot.process_id == process.id
                && Some(snapshot.pid) == process.pid
                && Some(&snapshot.incarnation) == incarnation.as_ref()
                && current_os_identity(snapshot.pid).ok().flatten().as_ref()
                    == Some(&snapshot.os_identity);
            let node_matches = observed_node(root, snapshot.target_root.as_deref()).ok()
                == Some(snapshot.node_id.clone());
            let fresh =
                snapshot.sequence > 0 && (0..SILENCE_MS).contains(&(now - snapshot.tick_ms));
            let cycle_fresh = snapshot
                .completed_cycle_ms
                .is_some_and(|t| (0..SILENCE_MS).contains(&(now - t)));
            let draining = !target_matches && !snapshot.active_attempts.is_empty();
            let healthy = runtime_matches
                && identity_matches
                && node_matches
                && (target_matches || draining)
                && fresh
                && cycle_fresh;
            let state = if healthy && draining {
                "draining"
            } else if healthy {
                "ticking"
            } else if !runtime_matches || !identity_matches || !node_matches {
                "unavailable"
            } else if !target_matches {
                "target_changed"
            } else if starting && snapshot.completed_cycle_ms.is_none() {
                "starting"
            } else {
                "stalled"
            };
            let reason = if healthy && draining {
                "scheduler is draining owned work from the previous target"
            } else if healthy {
                "workflow scheduler is ticking"
            } else if !target_matches {
                "worker has not observed the active target"
            } else {
                "scheduler ticks or admission cycles are stale, or worker identity is unverified"
            };
            WorkflowHealth {
                healthy,
                waiting_todo_count: 0,
                state: state.into(),
                reason: reason.into(),
                remedy: "refine system status; refine system doctor".into(),
                observation: Some(snapshot),
                admission: None,
            }
        }
        Err(error) => WorkflowHealth::unavailable(
            if starting { "starting" } else { "unavailable" },
            format!("scheduler evidence unavailable: {error}"),
        ),
    };
    if !health.healthy {
        health.remedy = format!(
            "refine system doctor; refine system status --port {}",
            root.file_name().and_then(|s| s.to_str()).unwrap_or("8080")
        );
    }
    health
}

pub fn assess_workflow_health(root: &Path) -> WorkflowHealth {
    let result = (|| -> RefineResult<WorkflowHealth> {
        if let Some(health) = ownership::assess(root)? {
            return Ok(health);
        }
        let supervisor = FileProcessSupervisor::new(root);
        let pause = supervisor.pause_state()?;
        if pause.disabled_background_workers.contains("workflow") {
            let mut health =
                WorkflowHealth::unavailable("disabled", "workflow automation is disabled");
            health.healthy = true;
            return Ok(health);
        }
        let workers = supervisor
            .list()?
            .into_iter()
            .filter(is_workflow_worker)
            .collect::<Vec<_>>();
        let mut live = Vec::new();
        for process in &workers {
            if supervisor.owned_process_is_alive(process)? {
                live.push(process.clone());
            }
        }
        let registry_root = workers
            .first()
            .and_then(|p| p.details.as_deref())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v["project_registry_root"].as_str().map(PathBuf::from))
            .unwrap_or_else(|| root.into());
        let target = crate::application::projects::registry::FileProjectRegistryService::new(
            &registry_root,
            None,
        )
        .load()?
        .active_app
        .map(PathBuf::from);
        let mut health = if live.len() == 1 {
            assess_worker(root, &live[0], target.as_deref())
        } else {
            WorkflowHealth::unavailable(
                "unavailable",
                format!("expected one workflow worker; observed {}", live.len()),
            )
        };
        if health.healthy && pause.workflow_paused {
            health.state = "paused".into();
            health.reason = "workflow is paused; scheduler is ticking".into();
        } else if health.healthy && health.state != "draining" && target.is_none() {
            health.state = "detached".into();
            health.reason = "no target app is attached; scheduler is ticking".into();
        }
        health.admission = admission::read_admission(
            root,
            target.as_deref(),
            chrono::Utc::now().timestamp_millis(),
        );
        if let Some(admission) = &health.admission {
            let waiting = admission.waiting_count(chrono::Utc::now().timestamp_millis());
            health.waiting_todo_count = waiting;
            if waiting > 0 && health.healthy {
                health.healthy = false;
                health.state = "admission_stalled".into();
                health.reason = format!(
                    "{waiting} continuously eligible Todo Goals have waited over 30 seconds with free capacity"
                );
            }
        }
        let recovery = root.join("workflow-recovery.json");
        if recovery.exists() {
            let value: serde_json::Value = serde_json::from_slice(
                &std::fs::read(recovery).map_err(|e| RefineError::Io(e.to_string()))?,
            )
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
            if value["pending"] != false {
                health.healthy = false;
                health.state = "recovering".into();
                health.reason = value["failure"]
                    .as_str()
                    .unwrap_or("replacement scheduler readiness is unconfirmed")
                    .into();
            }
        }
        Ok(health)
    })();
    result.unwrap_or_else(|error| WorkflowHealth::unavailable("unavailable", error.to_string()))
}

/// Surfaces keep the daemon's reachability fields and apply the same workflow degradation.
pub fn enrich_status(root: &Path, value: &mut serde_json::Value) {
    let health = assess_workflow_health(root);
    if let Some(object) = value.as_object_mut() {
        if !health.healthy {
            object.insert("daemon_healthy".into(), serde_json::json!(false));
        }
        object.insert("workflow_health".into(), serde_json::json!(health));
        let maintenance = crate::application::workers::maintenance::inspect_health(root);
        if maintenance["state"] == "unhealthy" {
            object.insert("daemon_healthy".into(), serde_json::json!(false));
        }
        object.insert("daemon_maintenance".into(), maintenance);
    }
}
