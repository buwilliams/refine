//! Runtime shutdown uses the same scope proof as maintenance and worker replacement.
use super::*;
use crate::infrastructure::process::subprocess::ManagedProcess;

const SCOPE_STOP_WAIT: Duration = Duration::from_secs(2);
const TERMINATE_WAIT: Duration = Duration::from_secs(2);
const KILL_WAIT: Duration = Duration::from_secs(1);

impl FileDaemonLifecycleService {
    pub(crate) fn stop_runtime_processes(&self, port: u16) -> RefineResult<()> {
        self.with_runtime_stop_lock(port, |supervisors| {
            #[cfg(target_os = "linux")]
            stop_owned_runtime(supervisors)?;
            #[cfg(not(target_os = "linux"))]
            stop_registered_runtime(supervisors)?;
            Ok(())
        })
    }

    /// Service managers own the daemon process. Drain its workflow scopes first
    /// and retain the supervision lease until their stop/restart command returns.
    pub(crate) fn with_runtime_workers_stopped<T>(
        &self,
        port: u16,
        control: impl FnOnce() -> RefineResult<T>,
    ) -> RefineResult<T> {
        self.with_runtime_stop_lock(port, |supervisors| {
            stop_runtime_workers(supervisors)?;
            control()
        })
    }

    fn with_runtime_stop_lock<T>(
        &self,
        port: u16,
        operation: impl FnOnce(&[FileProcessSupervisor]) -> RefineResult<T>,
    ) -> RefineResult<T> {
        let root = self.runtime_root.port_root(port);
        fs::create_dir_all(&root).map_err(|e| RefineError::Io(e.to_string()))?;
        // The daemon must not replace the workflow worker while its scopes drain.
        // This is the same lock used by ordinary workflow supervision.
        let path = root.join("workflow-supervision.lock");
        let supervision = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| RefineError::Io(e.to_string()))?;
        crate::infrastructure::process::supervisor::coordination::lock_exclusive_before(
            &supervision,
            &path,
            SCOPE_STOP_WAIT,
        )?;
        let supervisors = [
            FileProcessSupervisor::new(root.join("agents")),
            FileProcessSupervisor::new(root),
        ];
        operation(&supervisors)
    }
}

fn stop_runtime_workers(supervisors: &[FileProcessSupervisor]) -> RefineResult<()> {
    // An execution can register while its spawning worker is being stopped.
    // Rescan both registries after the worker's complete scope has exited.
    for _ in 0..2 {
        #[cfg(target_os = "linux")]
        stop_phase(supervisors, false)?;
        #[cfg(not(target_os = "linux"))]
        stop_registered_phase(supervisors, false)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop_owned_runtime(supervisors: &[FileProcessSupervisor]) -> RefineResult<()> {
    stop_runtime_workers(supervisors)?;
    stop_phase(supervisors, true)?;
    // Also discover any worker launched by the daemon during its final stop.
    stop_phase(supervisors, false)?;
    for supervisor in supervisors {
        for group in supervisor.owned_groups()? {
            if !supervisor.observe_owned_group(&group)?.confirmed_exit {
                return Err(RefineError::Degraded(format!(
                    "runtime shutdown left scope {} unverified; ownership evidence retained",
                    group.process.id
                )));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop_phase(supervisors: &[FileProcessSupervisor], daemon: bool) -> RefineResult<()> {
    let mut scopes = Vec::new();
    for supervisor in supervisors {
        for group in supervisor.owned_groups()? {
            if (group.process.owner == ProcessOwner::Daemon) == daemon {
                scopes.push((supervisor, group));
            }
        }
    }
    // Freeze the worker and its descendant workloads together. Stopping a child
    // first could let the still-running worker publish a failed execution result
    // during restart. Guardians survive the exact-identity stop to reap both scopes.
    scopes.sort_by_key(|(_, group)| group.process.owner != ProcessOwner::Runner);
    for (supervisor, group) in &scopes {
        supervisor.stop_owned_group(group, SCOPE_STOP_WAIT)?;
    }
    for supervisor in supervisors {
        let mut processes = supervisor.capacity_processes()?;
        processes.sort_by_key(|process| process.owner != ProcessOwner::Runner);
        for process in processes
            .into_iter()
            .filter(|process| (process.owner == ProcessOwner::Daemon) == daemon)
        {
            if FileProcessSupervisor::requires_group_ownership(&process)
                || scopes
                    .iter()
                    .any(|(_, group)| group.process.id == process.id)
            {
                // A missing registration or terminal leader never substitutes
                // for the owned scope's lifetime proof.
                if supervisor.group_pending(&process)? {
                    return Err(RefineError::Degraded(format!(
                        "scope {} remains pending during runtime shutdown",
                        process.id
                    )));
                }
            } else {
                stop_legacy_process(supervisor, &process)?;
            }
        }
    }
    Ok(())
}

// Platforms without complete scope inspection still support stopping known
// registered processes. This establishes only their PID exits; ownership
// records remain intact and must not be promoted to complete scope proof.
#[cfg(any(not(target_os = "linux"), test))]
fn stop_registered_runtime(supervisors: &[FileProcessSupervisor]) -> RefineResult<()> {
    for daemon in [false, true, false] {
        stop_registered_phase(supervisors, daemon)?;
    }
    Ok(())
}

#[cfg(any(not(target_os = "linux"), test))]
fn stop_registered_phase(supervisors: &[FileProcessSupervisor], daemon: bool) -> RefineResult<()> {
    let mut processes = Vec::new();
    for supervisor in supervisors {
        for process in supervisor.list()? {
            if (process.owner == ProcessOwner::Daemon) == daemon {
                processes.push((supervisor, process));
            }
        }
    }
    processes.sort_by_key(|(_, process)| process.owner != ProcessOwner::Runner);
    for (supervisor, process) in processes {
        if FileProcessSupervisor::requires_group_ownership(&process) {
            stop_registered_owned_process(supervisor, &process)?;
        } else {
            stop_legacy_process(supervisor, &process)?;
        }
    }
    Ok(())
}

#[cfg(any(not(target_os = "linux"), test))]
fn stop_registered_owned_process(
    supervisor: &FileProcessSupervisor,
    process: &ManagedProcess,
) -> RefineResult<()> {
    for (signal, timeout) in [("terminate", TERMINATE_WAIT), ("kill", KILL_WAIT)] {
        if !FileProcessSupervisor::process_is_alive(process)? {
            return Ok(());
        }
        let current = supervisor.inspect(&process.id)?;
        if current.pid != process.pid
            || current.owner != process.owner
            || current.started_at != process.started_at
        {
            return Err(RefineError::Conflict(format!(
                "process {} registration changed during shutdown",
                process.id
            )));
        }
        supervisor.request_termination(&process.id, signal)?;
        let deadline = Instant::now() + timeout;
        while FileProcessSupervisor::process_is_alive(process)? {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    if FileProcessSupervisor::process_is_alive(process)? {
        return Err(RefineError::Degraded(format!(
            "registered process {} did not exit during runtime shutdown; evidence retained",
            process.id
        )));
    }
    Ok(())
}

fn stop_legacy_process(
    supervisor: &FileProcessSupervisor,
    process: &ManagedProcess,
) -> RefineResult<()> {
    match supervisor.wait(&process.id) {
        Ok(observed) if observed.state != "running" => return Ok(()),
        Err(RefineError::NotFound(_)) => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    if process.pid.is_none() {
        // Legacy status-only registrations have no OS process to signal.
        supervisor.signal(&process.id, "terminate")?;
        return Ok(());
    }
    let exit =
        match supervisor.terminate_owned_and_confirm_exit(process, "terminate", TERMINATE_WAIT) {
            Ok(exit) => exit,
            Err(error) => {
                // Only the same still-live identity permits escalation. A missing or
                // changed registration retains the original stop failure.
                if !supervisor.owned_process_is_alive(process).unwrap_or(false) {
                    return Err(error);
                }
                supervisor.terminate_owned_and_confirm_exit(process, "kill", KILL_WAIT)?
            }
        };
    supervisor
        .cleanup_confirmed_exit(process, exit)
        .map_err(|failure| failure.error)?;
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    #[test]
    fn registered_process_fallback_stops_without_inventing_scope_exit() {
        let root = std::env::temp_dir().join(format!(
            "refine-registered-shutdown-{}",
            uuid::Uuid::new_v4()
        ));
        let supervisor = FileProcessSupervisor::new(&root);
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let process = supervisor
            .register(ManagedProcess {
                id: "registered-worker".into(),
                owner: ProcessOwner::Runner,
                pid: Some(child.id()),
                state: "running".into(),
                label: None,
                details: Some(
                    serde_json::json!({
                        "workflow_incarnation": "registered-only",
                        "isolated_process_group": true
                    })
                    .to_string(),
                ),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: chrono::Utc::now().to_rfc3339(),
                exit_code: None,
            })
            .unwrap();
        let group = supervisor.owned_groups().unwrap().remove(0);
        let path = root.join("owned-groups/registered-worker.json");
        let original = fs::read(&path).unwrap();
        let result = stop_registered_runtime(std::slice::from_ref(&supervisor));
        let _ = child.kill();
        child.wait().unwrap();
        result.unwrap();
        assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(supervisor.assess_owned_group(&group).unwrap().pending());
        assert!(
            supervisor
                .processes_dir()
                .join("registered-worker.json")
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
