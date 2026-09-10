//! Application health for incomplete execution ownership, including missing registrations.
use super::*;
use crate::infrastructure::process::subprocess::owned_groups::OwnershipAssessment;

pub(super) fn assess(root: &Path) -> RefineResult<Option<WorkflowHealth>> {
    for process_root in [root.to_path_buf(), root.join("agents")] {
        let owner = FileProcessSupervisor::new(&process_root);
        let groups = owner.owned_groups()?;
        for group in &groups {
            if let OwnershipAssessment::Unverified { reason } = owner.assess_owned_group(group)? {
                return Ok(Some(unverified(root, &group.process, &reason)));
            }
        }
        for process in owner.capacity_processes()? {
            if let Some(reason) = process
                .details
                .as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|v| v["scope_settlement_error"].as_str().map(str::to_owned))
            {
                return Ok(Some(unverified(root, &process, &reason)));
            }
            if FileProcessSupervisor::requires_group_ownership(&process)
                && !groups.iter().any(|g| g.process.id == process.id)
            {
                return Ok(Some(unverified(
                    root,
                    &process,
                    "owned-group evidence is missing",
                )));
            }
        }
    }
    Ok(None)
}

fn unverified(root: &Path, process: &ManagedProcess, reason: &str) -> WorkflowHealth {
    let mut health = WorkflowHealth::unavailable(
        "ownership_unverified",
        format!(
            "process {} (PID {:?}, started {}): {reason}; capacity and artifacts retained; replacement refused",
            process.id, process.pid, process.started_at
        ),
    );
    health.remedy = format!(
        "refine system status --port {}; refine system doctor --runtime-root {}",
        root.file_name().and_then(|s| s.to_str()).unwrap_or("8080"),
        root.parent().unwrap_or(root).display()
    );
    health
}
