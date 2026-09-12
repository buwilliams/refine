//! Application-owned worker replacement. The daemon calls this independently of admission.
use super::*;
use crate::application::workflow::health::assess_worker;
use crate::infrastructure::process::subprocess::scheduler_observation::{
    is_workflow_worker, workflow_incarnation,
};
use fs2::FileExt;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkflowRecovery {
    pending: bool,
    worker: ManagedProcess,
    stopped: bool,
    attempts: u32,
    retry_after_ms: i64,
    failure: String,
}
impl FileRunnerWorkerService {
    pub(super) fn ensure_workflow_worker(&self) -> RefineResult<BackgroundWorkerEnsure> {
        std::fs::create_dir_all(&self.runtime_root).map_err(|e| RefineError::Io(e.to_string()))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.runtime_root.join("workflow-supervision.lock"))
            .map_err(|e| RefineError::Io(e.to_string()))?;
        lock.try_lock_exclusive().map_err(|e| {
            RefineError::Degraded(format!("workflow supervision is already in progress: {e}"))
        })?;
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        if supervisor
            .pause_state()?
            .disabled_background_workers
            .contains(WORKFLOW_RUNNER)
        {
            return Ok(BackgroundWorkerEnsure::Disabled);
        }
        let target =
            current_target_root(&self.runtime_root, self.project_registry_root.as_deref())?;
        let processes = supervisor
            .capacity_processes()?
            .into_iter()
            .filter(is_workflow_worker)
            .collect::<Vec<_>>();
        let mut live = Vec::new();
        for process in &processes {
            if supervisor.group_pending(process)? {
                live.push(process.clone());
            }
        }
        if live.len() > 1 {
            return Err(RefineError::Degraded(
                "multiple workflow workers; replacement refused; inspect refine system status and refine system doctor"
                    .into(),
            ));
        }
        let path = self.runtime_root.join("workflow-recovery.json");
        let mut recovery: Option<WorkflowRecovery> = match std::fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|e| {
                RefineError::Serialization(format!("workflow recovery evidence: {e}"))
            })?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(worker) = live.first() {
            let health = assess_worker(&self.runtime_root, worker, target.as_deref());
            if health.healthy {
                let _ =
                    std::fs::remove_file(self.runtime_root.join("workflow-target-transition.json"));
                if let Some(record) = recovery.as_mut().filter(|r| r.pending) {
                    if worker.id == record.worker.id || !record.stopped {
                        return Err(RefineError::Degraded(
                            "old worker exit is not proved; replacement remains unhealthy".into(),
                        ));
                    }
                    record.pending = false;
                    // Backoff counts consecutive failed replacements, not every
                    // recovery in this installation's lifetime.
                    record.attempts = 0;
                    record.retry_after_ms = 0;
                    record.failure = "replacement scheduler tick verified".into();
                    write_recovery(&path, record)?;
                }
                return Ok(BackgroundWorkerEnsure::Running(Box::new(worker.clone())));
            }
            if health.state == "target_changed"
                && self.target_transition_pending(worker, target.as_deref(), now)?
            {
                return Ok(BackgroundWorkerEnsure::Running(Box::new(worker.clone())));
            }
            if health.state == "starting" {
                return Ok(BackgroundWorkerEnsure::Running(Box::new(worker.clone())));
            }
            if recovery
                .as_ref()
                .is_none_or(|r| !r.pending || r.stopped && r.worker.id != worker.id)
            {
                eprintln!(
                    "refine workflow recovery: worker={} state={} reason={}",
                    worker.id, health.state, health.reason
                );
                let attempts = recovery
                    .as_ref()
                    .filter(|r| r.pending)
                    .map(|r| r.attempts)
                    .unwrap_or(0);
                recovery = Some(WorkflowRecovery {
                    pending: true,
                    worker: worker.clone(),
                    stopped: false,
                    attempts,
                    retry_after_ms: 0,
                    failure: health.reason,
                });
            }
        }
        // An exited worker still owns registered children. It must go through the same fence.
        if recovery.as_ref().is_none_or(|r| !r.pending)
            && live.is_empty()
            && let Some(group) = supervisor
                .owned_groups()?
                .into_iter()
                .rev()
                .find(|g| is_workflow_worker(&g.process))
        {
            recovery = Some(WorkflowRecovery {
                pending: true,
                worker: group.process,
                stopped: false,
                attempts: recovery
                    .as_ref()
                    .filter(|r| r.pending)
                    .map(|r| r.attempts)
                    .unwrap_or(0),
                retry_after_ms: 0,
                failure: "workflow process exited; checking its owned executions".into(),
            });
        }
        if let Some(record) = recovery.as_mut().filter(|r| r.pending) {
            if now < record.retry_after_ms {
                return Err(RefineError::Degraded(format!(
                    "{}; retry after {}",
                    record.failure, record.retry_after_ms
                )));
            }
            if !record.stopped {
                record.attempts = record.attempts.saturating_add(1);
                record.retry_after_ms = now + restart_delay_ms(record.attempts);
                write_recovery(&path, record)?;
                match self.stop_workflow_incarnation(&record.worker) {
                    Ok(()) => {
                        record.stopped = true;
                        // Backoff throttles failed stops, not a successful ownership handoff.
                        // The next tick may launch; a failed launch sets its own delay.
                        record.retry_after_ms = 0;
                        record.failure =
                            "old worker and owned groups exited; awaiting replacement tick".into();
                    }
                    Err(error) => {
                        record.failure = format!(
                            "{error}; inspect refine system status and refine system doctor and resolve ownership or termination before retrying"
                        );
                        write_recovery(&path, record)?;
                        return Err(error);
                    }
                }
                write_recovery(&path, record)?;
                return Err(RefineError::Degraded(record.failure.clone()));
            }
        }
        // Recheck the launch gate under the same port-local fence. A replacement remains
        // unhealthy until a later daemon tick accepts its matching scheduling observation.
        if supervisor
            .pause_state()?
            .disabled_background_workers
            .contains(WORKFLOW_RUNNER)
        {
            return Ok(BackgroundWorkerEnsure::Disabled);
        }
        if let Some(worker) = live.first() {
            return Ok(BackgroundWorkerEnsure::Running(Box::new(worker.clone())));
        }
        // A retained group from another incarnation must not disappear behind the current
        // recovery record. Unknown or surviving ownership prevents replacement admission.
        for root in [self.runtime_root.clone(), self.runtime_root.join("agents")] {
            let owner = FileProcessSupervisor::new(root);
            for process in owner.capacity_processes()? {
                if workflow_incarnation(&process).is_some() && owner.group_pending(&process)? {
                    return Err(RefineError::Degraded("another workflow incarnation still owns work; inspect refine system status and refine system doctor before replacement".into()));
                }
            }
        }
        let launch = (|| {
            let executable = runner_executable(&self.runtime_root)?;
            supervisor.launch(background_worker_spec(
                &executable,
                &self.runtime_root,
                self.project_registry_root.as_deref(),
                WORKFLOW_RUNNER,
            ))
        })();
        match launch {
            Ok(process) => Ok(BackgroundWorkerEnsure::Running(Box::new(process))),
            Err(error) => {
                if let Some(record) = recovery.as_mut().filter(|r| r.pending) {
                    record.attempts = record.attempts.saturating_add(1);
                    record.retry_after_ms = now + restart_delay_ms(record.attempts);
                    record.failure = format!("replacement launch failed: {error}");
                    write_recovery(&path, record)?;
                }
                Err(error)
            }
        }
    }

    fn target_transition_pending(
        &self,
        worker: &ManagedProcess,
        target: Option<&Path>,
        now: i64,
    ) -> RefineResult<bool> {
        let path = self.runtime_root.join("workflow-target-transition.json");
        let previous: Option<Value> = match std::fs::read(&path) {
            Ok(bytes) => Some(
                serde_json::from_slice(&bytes)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        let target = target.map(|p| p.display().to_string());
        let since = previous
            .filter(|p| {
                p["process_id"] == worker.id
                    && p["incarnation"] == json!(workflow_incarnation(worker))
                    && p["target"] == json!(target)
            })
            .and_then(|p| p["since_ms"].as_i64())
            .unwrap_or(now);
        crate::infrastructure::process::subprocess::write_json_atomically(&path,
            &serde_json::to_vec(&json!({"process_id":worker.id,"incarnation":workflow_incarnation(worker),"target":target,"since_ms":since})).map_err(|e| RefineError::Serialization(e.to_string()))?, "workflow target transition")?;
        Ok((0..crate::application::workflow::health::STARTUP_MS).contains(&(now - since)))
    }

    fn stop_workflow_incarnation(&self, worker: &ManagedProcess) -> RefineResult<()> {
        let token = workflow_incarnation(worker).ok_or_else(|| {
            RefineError::Degraded("workflow incarnation unavailable; cannot replace safely".into())
        })?;
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        let worker_group = supervisor
            .owned_groups()?
            .into_iter()
            .find(|g| g.process.id == worker.id)
            .ok_or_else(|| {
                RefineError::Degraded("workflow group ownership evidence unavailable".into())
            })?;
        supervisor.stop_owned_group(&worker_group, Duration::from_secs(2))?;
        // The spawning worker is gone. Rescan registration-time groups, including those whose
        // leaders have already exited, before claiming that every owned execution is stopped.
        for root in [self.runtime_root.clone(), self.runtime_root.join("agents")] {
            let supervisor = FileProcessSupervisor::new(root);
            let groups = supervisor.owned_groups()?;
            for process in supervisor.list()? {
                if workflow_incarnation(&process).as_deref() == Some(&token)
                    && !groups.iter().any(|g| g.process.id == process.id)
                    && supervisor.group_pending(&process)?
                {
                    return Err(RefineError::Degraded(format!(
                        "process {} has no group evidence; replacement refused",
                        process.id
                    )));
                }
            }
            for group in groups
                .into_iter()
                .filter(|g| workflow_incarnation(&g.process).as_deref() == Some(&token))
            {
                supervisor.stop_owned_group(&group, Duration::from_secs(2))?;
            }
            for group in supervisor
                .owned_groups()?
                .into_iter()
                .filter(|g| workflow_incarnation(&g.process).as_deref() == Some(&token))
            {
                if !supervisor.observe_owned_group(&group)?.confirmed_exit {
                    return Err(RefineError::Degraded(
                        "owned descendant is still alive".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
fn restart_delay_ms(attempts: u32) -> i64 {
    (1_000_i64 * (1_i64 << attempts.saturating_sub(1).min(9))).min(300_000)
}
fn write_recovery(path: &Path, record: &WorkflowRecovery) -> RefineResult<()> {
    crate::infrastructure::process::subprocess::write_json_atomically(
        path,
        &serde_json::to_vec(record).map_err(|e| RefineError::Serialization(e.to_string()))?,
        "workflow recovery",
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
