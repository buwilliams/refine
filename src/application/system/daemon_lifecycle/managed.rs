use super::*;
use crate::infrastructure::process::supervisor::lifecycle::{
    DaemonLifecycleEvidence, DaemonRuntimeService,
};

pub(crate) fn run_service_managed_daemon(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    action: InstalledServiceAction,
    control: impl FnMut() -> RefineResult<()>,
) -> RefineResult<DaemonStatus> {
    run_service_managed_daemon_with(
        lifecycle,
        port,
        service_manager,
        action,
        std::time::Duration::from_secs(120),
        std::time::Duration::from_millis(100),
        control,
        http_reachability_probe,
    )
}

// The explicit timing and injected control/probe hooks are the failure-path test seam for this
// state machine; grouping them would obscure which side effect each test is replacing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_service_managed_daemon_with(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    action: InstalledServiceAction,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
    mut control: impl FnMut() -> RefineResult<()>,
    mut probe: impl FnMut(u16) -> DaemonReachability,
) -> RefineResult<DaemonStatus> {
    let action_name = managed_action_name(action)?;
    if !matches!(probe(port), DaemonReachability::Reachable) {
        lifecycle.begin_start(port)?;
    }
    if let Err(error) = control() {
        let observation = probe(port);
        persist_managed_command_failure(
            lifecycle,
            port,
            service_manager,
            action_name,
            &error,
            observation,
        )?;
        return Err(error);
    }
    wait_for_service_managed_daemon_with(
        lifecycle,
        port,
        service_manager,
        action,
        timeout,
        poll_interval,
        &mut probe,
    )
}

fn persist_managed_command_failure(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    action: &str,
    command_error: &RefineError,
    observation: DaemonReachability,
) -> RefineResult<DaemonStatus> {
    let (outcome, readiness_error, observed_reachable, recovery) = match observation {
        DaemonReachability::Reachable => (
            format!("{action}_command_failed_daemon_reachable"),
            None,
            Some(true),
            format!(
                "retry system {action} after correcting the service-manager command failure; the daemon remains reachable"
            ),
        ),
        DaemonReachability::Unreachable(_) => (
            format!("{action}_command_failed_daemon_unreachable"),
            None,
            Some(false),
            format!(
                "inspect the service-manager failure and restore the daemon before retrying system {action}"
            ),
        ),
        DaemonReachability::Unknown(error) => (
            format!("{action}_command_failed_daemon_reachability_unknown"),
            Some(error),
            None,
            format!(
                "the service-manager command may have partially changed daemon state; inspect {service_manager} and daemon reachability before retrying system {action}"
            ),
        ),
    };
    let evidence = DaemonLifecycleEvidence {
        action: action.to_string(),
        service_manager: service_manager.to_string(),
        outcome,
        command_error: Some(command_error.to_string()),
        readiness_error,
        observed_reachable,
        recovery: Some(recovery),
    };
    if evidence.observed_reachable == Some(true) {
        lifecycle.mark_observed_ready_with_evidence(port, Some(evidence))
    } else {
        lifecycle.mark_observed_failed_with_evidence(
            port,
            format!("{action}-failed:{command_error}"),
            Some(evidence),
        )
    }
}

pub(crate) fn stop_service_managed_daemon(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    control: impl FnMut() -> RefineResult<()>,
) -> RefineResult<DaemonStatus> {
    stop_service_managed_daemon_with(
        lifecycle,
        port,
        service_manager,
        std::time::Duration::from_secs(120),
        std::time::Duration::from_millis(100),
        control,
        http_reachability_probe,
    )
}

pub(crate) fn stop_service_managed_daemon_with(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
    mut control: impl FnMut() -> RefineResult<()>,
    mut probe: impl FnMut(u16) -> DaemonReachability,
) -> RefineResult<DaemonStatus> {
    if let Err(error) = control() {
        let observation = probe(port);
        let (outcome, readiness_error, observed_reachable, recovery) = match observation {
            DaemonReachability::Reachable => (
                "stop_command_failed_daemon_reachable",
                None,
                Some(true),
                "retry system stop after correcting the service-manager command failure",
            ),
            DaemonReachability::Unreachable(_) => (
                "stop_command_failed_shutdown_confirmed",
                None,
                Some(false),
                "shutdown was observed despite the command failure; inspect the service manager before restarting",
            ),
            DaemonReachability::Unknown(error) => (
                "stop_command_failed_daemon_reachability_unknown",
                Some(error),
                None,
                "the service-manager command may have partially changed daemon state; inspect the service manager and daemon reachability before retrying system stop",
            ),
        };
        let evidence = DaemonLifecycleEvidence {
            action: "stop".to_string(),
            service_manager: service_manager.to_string(),
            outcome: outcome.to_string(),
            command_error: Some(error.to_string()),
            readiness_error,
            observed_reachable,
            recovery: Some(recovery.to_string()),
        };
        match evidence.observed_reachable {
            Some(true) => {
                lifecycle.mark_observed_ready_with_evidence(port, Some(evidence))?;
            }
            Some(false) => {
                lifecycle.stop_runtime(port)?;
                lifecycle.mark_observed_stopped(port, Some(evidence))?;
            }
            None => {
                lifecycle.mark_observed_failed_with_evidence(
                    port,
                    format!("stop-failed:{error}"),
                    Some(evidence),
                )?;
            }
        }
        return Err(error);
    }

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match probe(port) {
            DaemonReachability::Unreachable(_) => {
                lifecycle.stop_runtime(port)?;
                return lifecycle.mark_observed_stopped(
                    port,
                    Some(DaemonLifecycleEvidence {
                        action: "stop".to_string(),
                        service_manager: service_manager.to_string(),
                        outcome: "stopped_confirmed".to_string(),
                        command_error: None,
                        readiness_error: None,
                        observed_reachable: Some(false),
                        recovery: None,
                    }),
                );
            }
            DaemonReachability::Reachable => {
                if std::time::Instant::now() >= deadline {
                    let error = RefineError::Degraded(format!(
                        "{service_manager} accepted the Refine stop command, but the daemon remained reachable on 127.0.0.1:{port}"
                    ));
                    let evidence = DaemonLifecycleEvidence {
                        action: "stop".to_string(),
                        service_manager: service_manager.to_string(),
                        outcome: "stop_timeout_daemon_reachable".to_string(),
                        command_error: None,
                        readiness_error: Some(error.to_string()),
                        observed_reachable: Some(true),
                        recovery: Some(
                            "retry system stop or inspect the service manager; the daemon remains active"
                                .to_string(),
                        ),
                    };
                    lifecycle.mark_observed_ready_with_evidence(port, Some(evidence))?;
                    return Err(error);
                }
            }
            DaemonReachability::Unknown(probe_error) => {
                if std::time::Instant::now() >= deadline {
                    let error = RefineError::Degraded(format!(
                        "{service_manager} accepted the Refine stop command, but shutdown confirmation is unknown on 127.0.0.1:{port} ({probe_error})"
                    ));
                    let evidence = DaemonLifecycleEvidence {
                        action: "stop".to_string(),
                        service_manager: service_manager.to_string(),
                        outcome: "stop_confirmation_unknown".to_string(),
                        command_error: None,
                        readiness_error: Some(error.to_string()),
                        observed_reachable: None,
                        recovery: Some(
                            "inspect the service manager and daemon reachability before retrying system stop"
                                .to_string(),
                        ),
                    };
                    lifecycle.mark_observed_failed_with_evidence(
                        port,
                        format!("stop-failed:{error}"),
                        Some(evidence),
                    )?;
                    return Err(error);
                }
            }
        }
        std::thread::sleep(poll_interval);
    }
}

fn wait_for_service_managed_daemon_with(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    action: InstalledServiceAction,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
    mut probe: impl FnMut(u16) -> DaemonReachability,
) -> RefineResult<DaemonStatus> {
    let action_name = managed_action_name(action)?;
    // A service-managed daemon reports startup through a published progress
    // token rather than through stderr, which goes to the journal where this
    // waiter cannot read it. Treating the budget as a stall budget means a slow
    // host takes longer instead of being reported as having failed to start.
    let mut progress = ReadinessProgress::new(timeout, std::time::Instant::now());
    let mut last_token = lifecycle.startup_progress_token(port);
    loop {
        let observation = probe(port);
        if observation == DaemonReachability::Reachable {
            return lifecycle.mark_observed_ready_with_evidence(
                port,
                Some(DaemonLifecycleEvidence {
                    action: action_name.to_string(),
                    service_manager: service_manager.to_string(),
                    outcome: format!("{action_name}_completed_daemon_reachable"),
                    command_error: None,
                    readiness_error: None,
                    observed_reachable: Some(true),
                    recovery: None,
                }),
            );
        }
        let status = lifecycle.status(port)?;
        if status.worker_state == "failed" {
            let error = RefineError::Degraded(format!(
                "{service_manager} accepted system {action_name} for the Refine service on port {port}, but the daemon reported {action_name} failure ({})",
                status.degraded_integrations.join(", ")
            ));
            persist_managed_readiness_failure(
                lifecycle,
                port,
                service_manager,
                action_name,
                &error,
                observation,
                format!("{action_name}_daemon_reported_failure"),
            )?;
            return Err(error);
        }
        let token = lifecycle.startup_progress_token(port);
        if token != last_token {
            last_token = token;
            progress.record_progress(std::time::Instant::now());
        }
        if progress.stalled(std::time::Instant::now()) {
            let (error, outcome) = match &observation {
                DaemonReachability::Unreachable(_) => (
                    RefineError::Degraded(format!(
                        "{service_manager} accepted system {action_name}, but the daemon did not become reachable on 127.0.0.1:{port}"
                    )),
                    format!("{action_name}_readiness_timeout_daemon_unreachable"),
                ),
                DaemonReachability::Unknown(probe_error) => (
                    RefineError::Degraded(format!(
                        "{service_manager} accepted system {action_name}, but daemon reachability is unknown on 127.0.0.1:{port} ({probe_error})"
                    )),
                    format!("{action_name}_readiness_probe_unknown"),
                ),
                DaemonReachability::Reachable => unreachable!("reachable returned above"),
            };
            persist_managed_readiness_failure(
                lifecycle,
                port,
                service_manager,
                action_name,
                &error,
                observation,
                outcome,
            )?;
            return Err(error);
        }
        std::thread::sleep(poll_interval);
    }
}

fn persist_managed_readiness_failure(
    lifecycle: &FileDaemonLifecycleService,
    port: u16,
    service_manager: &str,
    action: &str,
    error: &RefineError,
    observation: DaemonReachability,
    outcome: String,
) -> RefineResult<DaemonStatus> {
    let observed_reachable = match observation {
        DaemonReachability::Reachable => Some(true),
        DaemonReachability::Unreachable(_) => Some(false),
        DaemonReachability::Unknown(_) => None,
    };
    lifecycle.mark_observed_failed_with_evidence(
        port,
        format!("{action}-failed:{error}"),
        Some(DaemonLifecycleEvidence {
            action: action.to_string(),
            service_manager: service_manager.to_string(),
            outcome,
            command_error: None,
            readiness_error: Some(error.to_string()),
            observed_reachable,
            recovery: Some(format!(
                "inspect {service_manager} and daemon logs, then retry system {action}"
            )),
        }),
    )
}

fn managed_action_name(action: InstalledServiceAction) -> RefineResult<&'static str> {
    match action {
        InstalledServiceAction::Start => Ok("start"),
        InstalledServiceAction::Restart => Ok("restart"),
        InstalledServiceAction::Stop => Err(RefineError::InvalidInput(
            "managed start helper does not accept stop".to_string(),
        )),
    }
}
