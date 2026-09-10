//! Shared, transient admission bookkeeping for Goal workers and standalone Skills.
use crate::application::workflow::{WorkflowEngine, WorkflowPolicy};
use crate::error::RefineResult;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
pub(crate) struct ExecutionReservation {
    pub runtime: PathBuf,
    pub invocation_id: Option<String>,
    pub goal_id: Option<String>,
    pub node: String,
    pub provider: String,
    pub target: String,
}
static ACTIVE: OnceLock<Mutex<BTreeMap<String, ExecutionReservation>>> = OnceLock::new();
static ADMISSION: Mutex<()> = Mutex::new(());
pub(crate) fn reservations(runtime: &Path) -> Vec<ExecutionReservation> {
    ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter(|v| v.runtime == runtime)
        .cloned()
        .collect()
}

pub(crate) struct AdmissionLease(String);
impl Drop for AdmissionLease {
    fn drop(&mut self) {
        ACTIVE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

pub(crate) fn reserve(
    engine: &WorkflowEngine,
    policy: &WorkflowPolicy,
    key: String,
    request: ExecutionReservation,
) -> RefineResult<Option<AdmissionLease>> {
    let _admission = ADMISSION.lock().unwrap_or_else(|e| e.into_inner());
    engine.ensure_automation_running()?;
    if ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&key)
    {
        return Ok(None);
    }
    if !engine.soft_capacity_available(policy, &request.node, &request.provider, &request.target)? {
        return Ok(None);
    }
    ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.clone(), request);
    Ok(Some(AdmissionLease(key)))
}

/// Preserve alternating admission opportunities even when a pass has no active Goals.
pub(crate) fn skills_first(runtime: &Path) -> bool {
    static TURN: OnceLock<Mutex<BTreeMap<PathBuf, bool>>> = OnceLock::new();
    let mut turns = TURN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let next = turns.entry(runtime.into()).or_insert(true);
    let result = *next;
    *next = !result;
    result
}

impl ExecutionReservation {
    pub(crate) fn covers_process(&self, details: &serde_json::Value) -> bool {
        if self
            .invocation_id
            .as_deref()
            .is_some_and(|id| details["event_invocation_id"].as_str() == Some(id))
        {
            return true;
        }
        self.goal_id
            .as_deref()
            .is_some_and(|id| details["goal_id"].as_str() == Some(id))
            && details["workflow_revision"].is_u64()
            && details["target_app_id"].as_str() == Some(self.target.as_str())
            && details["node_id"].as_str() == Some(self.node.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn a_goal_slot_does_not_hide_independent_manual_work_for_that_goal() {
        let reservation = ExecutionReservation {
            runtime: PathBuf::from("runtime"),
            invocation_id: None,
            goal_id: Some("GOAL".into()),
            node: "node".into(),
            provider: "provider".into(),
            target: "target".into(),
        };
        let mut process = json!({"goal_id":"GOAL","node_id":"node","target_app_id":"target"});
        assert!(!reservation.covers_process(&process));
        process["workflow_revision"] = json!(3);
        assert!(reservation.covers_process(&process));
        process["target_app_id"] = json!("other-target");
        assert!(!reservation.covers_process(&process));
    }
}
