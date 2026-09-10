//! Capacity combines live reservations and retained execution scopes without hiding manual work.
use super::*;
impl WorkflowEngine {
    pub(crate) fn observed_execution_load(&self) -> RefineResult<ExecutionLoad> {
        let mut load = ExecutionLoad::default();
        let reservations =
            crate::application::workflow::engine::admission::reservations(&self.runtime_root);
        for r in &reservations {
            load.record(&r.node, &r.provider, &r.target);
        }
        let policy = self.policy()?;
        let mut processes = Vec::new();
        for root in [self.runtime_root.clone(), self.runtime_root.join("agents")] {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.capacity_processes()? {
                if supervisor.group_pending(&process)? {
                    let details = process
                        .details
                        .as_deref()
                        .and_then(|d| serde_json::from_str::<Value>(d).ok())
                        .unwrap_or(Value::Null);
                    processes.push((process, details));
                }
            }
        }
        let mut internal = std::collections::BTreeSet::new();
        for (process, details) in &processes {
            if reservations.iter().any(|r| r.covers_process(details)) {
                continue;
            }
            if !matches!(process.owner, ProcessOwner::Agent | ProcessOwner::Quality) {
                let Some(goal) = details["goal_id"].as_str() else {
                    continue;
                };
                let Some(incarnation) = details["workflow_incarnation"].as_str() else {
                    continue;
                };
                // Only an explicit workflow claim can associate an internal child with an
                // agent slot. A manual process mentioning the same Goal remains independent.
                if details["workflow_revision"].is_u64()
                    && processes.iter().any(|(p, d)| {
                        matches!(p.owner, ProcessOwner::Agent | ProcessOwner::Quality)
                            && d["goal_id"] == details["goal_id"]
                            && d["workflow_revision"] == details["workflow_revision"]
                            && d["workflow_incarnation"] == details["workflow_incarnation"]
                    })
                {
                    continue;
                }
                if !internal.insert((
                    goal.to_string(),
                    incarnation.to_string(),
                    details["workflow_revision"].as_u64(),
                )) {
                    continue;
                }
            }
            load.record(
                details["node_id"]
                    .as_str()
                    .unwrap_or(&policy.active_node_id),
                details["provider"].as_str().unwrap_or(&policy.provider),
                details["target_app_id"]
                    .as_str()
                    .or_else(|| details["cwd"].as_str())
                    .unwrap_or(&policy.target_app_id),
            );
        }
        Ok(load)
    }
}
