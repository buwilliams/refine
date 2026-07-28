use std::thread;
use std::time::Duration;

use crate::process::subprocess::{FileProcessSupervisor, ProcessOwner, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    DaemonLifecycleEvidence, DaemonReachability, DaemonRuntimeService, DaemonStatus,
    FileDaemonLifecycleService, http_reachability_probe,
};

pub(super) fn stop_direct_daemon(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
) -> RefineResult<DaemonStatus> {
    stop_direct_daemon_with(
        lifecycle,
        port,
        || stop_direct_runtime(lifecycle, port),
        http_reachability_probe,
    )
}

fn stop_direct_runtime(lifecycle: &FileDaemonLifecycleService, port: u16) -> RefineResult<()> {
    let supervisor = FileProcessSupervisor::new(lifecycle.runtime_root.port_root(port));
    let mut processes = supervisor.list()?;
    processes.sort_by_key(|process| process.owner == ProcessOwner::Daemon);
    let mut failures = Vec::new();
    for process in processes {
        let observed = match supervisor.wait(&process.id) {
            Ok(observed) => observed,
            Err(error) => {
                failures.push(format!("failed to inspect process {}: {error}", process.id));
                continue;
            }
        };
        if observed.state != "running" {
            continue;
        }
        if let Err(error) = supervisor.signal(&process.id, "terminate") {
            failures.push(format!(
                "failed to terminate process {}: {error}",
                process.id
            ));
            continue;
        }
        thread::sleep(Duration::from_millis(100));
        let still_running = match supervisor.wait(&process.id) {
            Ok(observed) => observed.state == "running",
            Err(error) => {
                failures.push(format!(
                    "failed to confirm process {} after terminate: {error}",
                    process.id
                ));
                continue;
            }
        };
        if !still_running {
            continue;
        }
        if let Err(error) = supervisor.signal(&process.id, "kill") {
            failures.push(format!("failed to kill process {}: {error}", process.id));
            continue;
        }
        thread::sleep(Duration::from_millis(100));
        match supervisor.wait(&process.id) {
            Ok(observed) if observed.state == "running" => failures.push(format!(
                "process {} remained running after terminate and kill",
                process.id
            )),
            Ok(_) => {}
            Err(error) => failures.push(format!(
                "failed to confirm process {} after kill: {error}",
                process.id
            )),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(RefineError::Degraded(format!(
            "direct daemon shutdown failed: {}",
            failures.join("; ")
        )))
    }
}

fn stop_direct_daemon_with(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    mut control: impl FnMut() -> RefineResult<()>,
    mut probe: impl FnMut(u16) -> DaemonReachability,
) -> RefineResult<DaemonStatus> {
    let control_result = control();
    let observation = probe(port);
    let command_error = control_result.as_ref().err().map(ToString::to_string);
    match observation {
        DaemonReachability::Unreachable(_) => {
            let outcome = if command_error.is_some() {
                "direct_stop_control_failed_shutdown_confirmed"
            } else {
                "direct_stop_confirmed"
            };
            lifecycle.mark_observed_stopped(
                port,
                Some(DaemonLifecycleEvidence {
                    action: "stop".to_string(),
                    service_manager: "direct_process".to_string(),
                    outcome: outcome.to_string(),
                    command_error,
                    readiness_error: None,
                    observed_reachable: Some(false),
                    recovery: control_result.as_ref().err().map(|_| {
                        "shutdown was observed, but process control reported an error; inspect the process registry before restarting"
                            .to_string()
                    }),
                }),
            )?;
            control_result
                .map(|_| lifecycle.status(port))
                .and_then(|status| status)
        }
        DaemonReachability::Reachable => {
            let readiness_error =
                format!("daemon remained reachable on 127.0.0.1:{port} after direct stop");
            let error = control_result.err().unwrap_or_else(|| {
                RefineError::Degraded(format!(
                    "direct daemon shutdown completed without a process-control error, but the daemon remained reachable on 127.0.0.1:{port}"
                ))
            });
            lifecycle.mark_observed_ready_with_evidence(
                port,
                Some(DaemonLifecycleEvidence {
                    action: "stop".to_string(),
                    service_manager: "direct_process".to_string(),
                    outcome: "direct_stop_daemon_reachable".to_string(),
                    command_error,
                    readiness_error: Some(readiness_error),
                    observed_reachable: Some(true),
                    recovery: Some(
                        "inspect the recorded daemon process and retry system stop; the daemon remains reachable"
                            .to_string(),
                    ),
                }),
            )?;
            Err(error)
        }
        DaemonReachability::Unknown(probe_error) => {
            let error = control_result.err().unwrap_or_else(|| {
                RefineError::Degraded(format!(
                    "direct daemon shutdown reachability is unknown on 127.0.0.1:{port}: {probe_error}"
                ))
            });
            lifecycle.mark_observed_failed_with_evidence(
                port,
                format!("stop-failed:{error}"),
                Some(DaemonLifecycleEvidence {
                    action: "stop".to_string(),
                    service_manager: "direct_process".to_string(),
                    outcome: "direct_stop_reachability_unknown".to_string(),
                    command_error: Some(error.to_string()),
                    readiness_error: Some(probe_error),
                    observed_reachable: None,
                    recovery: Some(
                        "process control may have partially stopped the daemon; inspect the process registry and daemon reachability before retrying"
                            .to_string(),
                    ),
                }),
            )?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::supervisor::lifecycle::{DaemonRuntimeService, running_status};
    use crate::process::supervisor::runtime::RuntimeRoot;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn surviving_direct_runtime_is_not_persisted_as_stopped() {
        let (root, lifecycle) = lifecycle("reachable");
        lifecycle.mark_ready(running_status(4557)).unwrap();
        let error = stop_direct_daemon_with(
            &lifecycle,
            4557,
            || Ok(()),
            |_| DaemonReachability::Reachable,
        )
        .unwrap_err();
        assert!(error.to_string().contains("remained reachable"));
        let status = lifecycle.status(4557).unwrap();
        assert_eq!(status.worker_state, "idle");
        assert_eq!(
            status.lifecycle_evidence.unwrap().observed_reachable,
            Some(true)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn control_error_and_unknown_probe_persist_partial_evidence() {
        let (root, lifecycle) = lifecycle("unknown");
        let error = stop_direct_daemon_with(
            &lifecycle,
            4557,
            || Err(RefineError::Degraded("terminate denied".to_string())),
            |_| DaemonReachability::Unknown("probe timed out".to_string()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "terminate denied");
        let status = lifecycle.status(4557).unwrap();
        assert_eq!(status.worker_state, "failed");
        let evidence = status.lifecycle_evidence.unwrap();
        assert_eq!(evidence.observed_reachable, None);
        assert_eq!(evidence.command_error.as_deref(), Some("terminate denied"));
        assert_eq!(evidence.readiness_error.as_deref(), Some("probe timed out"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn control_error_is_returned_even_when_shutdown_is_observed() {
        let (root, lifecycle) = lifecycle("control-error");
        let error = stop_direct_daemon_with(
            &lifecycle,
            4557,
            || Err(RefineError::Degraded("kill denied".to_string())),
            |_| DaemonReachability::Unreachable("connection refused".to_string()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "kill denied");
        let status = lifecycle.status(4557).unwrap();
        assert_eq!(status.worker_state, "stopped");
        assert_eq!(
            status.lifecycle_evidence.unwrap().observed_reachable,
            Some(false)
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn lifecycle(name: &str) -> (PathBuf, FileDaemonLifecycleService) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "refine-direct-stop-{name}-{}-{nonce}",
            std::process::id()
        ));
        let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot { root: root.clone() });
        (root, lifecycle)
    }
}
