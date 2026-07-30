use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::lifecycle::{DaemonRuntimeService, FileDaemonLifecycleService};

#[test]
fn restart_command_failure_reprobes_and_records_partial_launchd_shutdown() {
    let temp_root = unique_temp_dir("cli-system-restart-partial-launchd-shutdown");
    let runtime_root = temp_root.join("run");
    let port = 4557;
    let lifecycle = ready_lifecycle(&runtime_root, port);
    let probe_calls = std::cell::Cell::new(0);

    let error = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "launchd_login_item",
        InstalledServiceAction::Restart,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        || {
            Err(RefineError::Degraded(
                "launchctl bootstrap failed after legacy bootout".to_string(),
            ))
        },
        |_| {
            probe_calls.set(probe_calls.get() + 1);
            if probe_calls.get() == 1 {
                DaemonReachability::Reachable
            } else {
                DaemonReachability::Unreachable(
                    "connection refused after partial launchd migration".to_string(),
                )
            }
        },
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "launchctl bootstrap failed after legacy bootout"
    );
    assert_eq!(probe_calls.get(), 2);
    let status = lifecycle.status(port).unwrap();
    assert!(!status.daemon_healthy);
    assert!(!status.web_available);
    assert_eq!(status.worker_state, "failed");
    assert!(status.active_operations.is_empty());
    assert!(
        status.degraded_integrations.iter().any(
            |detail| detail == "restart-failed:launchctl bootstrap failed after legacy bootout"
        ),
        "{status:?}"
    );
    let evidence = status.lifecycle_evidence.unwrap();
    assert_eq!(evidence.action, "restart");
    assert_eq!(evidence.service_manager, "launchd_login_item");
    assert_eq!(
        evidence.outcome,
        "restart_command_failed_daemon_unreachable"
    );
    assert_eq!(
        evidence.command_error.as_deref(),
        Some("launchctl bootstrap failed after legacy bootout")
    );
    assert_eq!(evidence.readiness_error, None);
    assert_eq!(evidence.observed_reachable, Some(false));
    assert_eq!(
        evidence.recovery.as_deref(),
        Some(
            "inspect the service-manager failure and restore the daemon before retrying system restart"
        )
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn restart_command_failure_reprobes_and_preserves_observed_reachable_health() {
    let temp_root = unique_temp_dir("cli-system-restart-failure-still-reachable");
    let runtime_root = temp_root.join("run");
    let port = 4557;
    let lifecycle = ready_lifecycle(&runtime_root, port);
    let probe_calls = std::cell::Cell::new(0);

    let error = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "systemd_user",
        InstalledServiceAction::Restart,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        || {
            Err(RefineError::Degraded(
                "systemctl restart returned a failure".to_string(),
            ))
        },
        |_| {
            probe_calls.set(probe_calls.get() + 1);
            DaemonReachability::Reachable
        },
    )
    .unwrap_err();

    assert_eq!(probe_calls.get(), 2);
    assert_eq!(error.to_string(), "systemctl restart returned a failure");
    let status = lifecycle.status(port).unwrap();
    assert!(status.daemon_healthy);
    assert!(status.web_available);
    assert_eq!(status.worker_state, "idle");
    let evidence = status.lifecycle_evidence.unwrap();
    assert_eq!(evidence.action, "restart");
    assert_eq!(evidence.outcome, "restart_command_failed_daemon_reachable");
    assert_eq!(evidence.observed_reachable, Some(true));
    assert_eq!(
        evidence.recovery.as_deref(),
        Some(
            "retry system restart after correcting the service-manager command failure; the daemon remains reachable"
        )
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn command_failure_probe_error_or_timeout_fails_closed_with_unknown_evidence() {
    let temp_root = unique_temp_dir("cli-system-command-failure-unknown-reprobe");
    let runtime_root = temp_root.join("run");

    for (offset, probe_error) in [
        "post-failure probe returned malformed HTTP",
        "post-failure probe timed out",
    ]
    .into_iter()
    .enumerate()
    {
        let port = 4557 + u16::try_from(offset).unwrap();
        let lifecycle = ready_lifecycle(&runtime_root, port);
        let probe_calls = std::cell::Cell::new(0);
        let error = run_service_managed_daemon_with(
            &lifecycle,
            port,
            "launchd_login_item",
            InstalledServiceAction::Restart,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            || {
                Err(RefineError::Degraded(
                    "launchctl restart sequence failed".to_string(),
                ))
            },
            |_| {
                probe_calls.set(probe_calls.get() + 1);
                if probe_calls.get() == 1 {
                    DaemonReachability::Reachable
                } else {
                    DaemonReachability::Unknown(probe_error.to_string())
                }
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "launchctl restart sequence failed");
        assert_eq!(probe_calls.get(), 2);
        let status = lifecycle.status(port).unwrap();
        assert!(!status.daemon_healthy);
        assert!(!status.web_available);
        assert_eq!(status.worker_state, "failed");
        let evidence = status.lifecycle_evidence.unwrap();
        assert_eq!(evidence.action, "restart");
        assert_eq!(
            evidence.outcome,
            "restart_command_failed_daemon_reachability_unknown"
        );
        assert_eq!(evidence.observed_reachable, None);
        assert_eq!(evidence.readiness_error.as_deref(), Some(probe_error));
        assert!(
            evidence
                .recovery
                .as_deref()
                .unwrap()
                .contains("may have partially changed daemon state"),
            "{evidence:?}"
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn initially_unreachable_start_failure_uses_post_failure_observation() {
    let temp_root = unique_temp_dir("cli-system-initially-unreachable-start-failure");
    let runtime_root = temp_root.join("run");
    let port = 4557;
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    });
    let probe_calls = std::cell::Cell::new(0);

    let error = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "systemd_user",
        InstalledServiceAction::Start,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        || {
            Err(RefineError::Degraded(
                "systemctl start failed while inactive".to_string(),
            ))
        },
        |_| {
            probe_calls.set(probe_calls.get() + 1);
            DaemonReachability::Unreachable("connection refused".to_string())
        },
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "systemctl start failed while inactive");
    assert_eq!(probe_calls.get(), 2);
    let status = lifecycle.status(port).unwrap();
    assert!(!status.daemon_healthy);
    assert!(!status.web_available);
    assert_eq!(status.worker_state, "failed");
    let evidence = status.lifecycle_evidence.unwrap();
    assert_eq!(evidence.action, "start");
    assert_eq!(evidence.outcome, "start_command_failed_daemon_unreachable");
    assert_eq!(evidence.observed_reachable, Some(false));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn successful_restart_records_restart_specific_evidence() {
    let temp_root = unique_temp_dir("cli-system-successful-managed-restart");
    let runtime_root = temp_root.join("run");
    let port = 4557;
    let lifecycle = ready_lifecycle(&runtime_root, port);

    let status = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "systemd_user",
        InstalledServiceAction::Restart,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        || Ok(()),
        |_| DaemonReachability::Reachable,
    )
    .unwrap();

    assert!(status.daemon_healthy);
    assert!(status.web_available);
    assert_eq!(status.worker_state, "idle");
    let evidence = status.lifecycle_evidence.unwrap();
    assert_eq!(evidence.action, "restart");
    assert_eq!(evidence.service_manager, "systemd_user");
    assert_eq!(evidence.outcome, "restart_completed_daemon_reachable");
    assert_eq!(evidence.command_error, None);
    assert_eq!(evidence.readiness_error, None);
    assert_eq!(evidence.observed_reachable, Some(true));
    assert_eq!(evidence.recovery, None);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn restart_readiness_timeout_records_restart_specific_evidence_and_log() {
    let temp_root = unique_temp_dir("cli-system-managed-restart-timeout");
    let runtime_root = temp_root.join("run");
    let port = 4557;
    let lifecycle = ready_lifecycle(&runtime_root, port);
    let probe_calls = std::cell::Cell::new(0);

    let error = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "launchd_login_item",
        InstalledServiceAction::Restart,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        || Ok(()),
        |_| {
            probe_calls.set(probe_calls.get() + 1);
            if probe_calls.get() == 1 {
                DaemonReachability::Reachable
            } else {
                DaemonReachability::Unreachable("connection refused after restart".to_string())
            }
        },
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("launchd_login_item accepted system restart"),
        "{error}"
    );
    let status = lifecycle.status(port).unwrap();
    assert!(!status.daemon_healthy);
    assert!(!status.web_available);
    assert_eq!(status.worker_state, "failed");
    assert!(
        status
            .degraded_integrations
            .iter()
            .any(|detail| detail.starts_with("restart-failed:")),
        "{status:?}"
    );
    assert!(
        status
            .degraded_integrations
            .iter()
            .all(|detail| !detail.starts_with("startup-failed:")),
        "{status:?}"
    );
    let evidence = status.lifecycle_evidence.unwrap();
    assert_eq!(evidence.action, "restart");
    assert_eq!(
        evidence.outcome,
        "restart_readiness_timeout_daemon_unreachable"
    );
    assert_eq!(evidence.command_error, None);
    assert!(
        evidence
            .readiness_error
            .as_deref()
            .unwrap()
            .contains("accepted system restart")
    );
    assert_eq!(evidence.observed_reachable, Some(false));
    assert_eq!(
        evidence.recovery.as_deref(),
        Some("inspect launchd_login_item and daemon logs, then retry system restart")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

fn ready_lifecycle(runtime_root: &Path, port: u16) -> FileDaemonLifecycleService {
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.to_path_buf(),
    });
    let status = lifecycle.recover(port).unwrap();
    lifecycle.mark_ready(status).unwrap();
    lifecycle
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nonce}", std::process::id()))
}

// A service-managed daemon writes its startup output to the journal, where this
// waiter cannot read it, so its readiness budget had nothing to observe and a
// slow start on a constrained host was reported as a failure to start. Startup
// milestones are published instead, and reporting one resets the budget.
#[test]
fn managed_readiness_waits_while_startup_keeps_reporting_progress() {
    let temp_root = unique_temp_dir("cli-system-managed-progress");
    let runtime_root = temp_root.join("run");
    let port = 4561;
    let lifecycle = ready_lifecycle(&runtime_root, port);
    let probe_calls = std::cell::Cell::new(0);

    // Startup runs well past the budget in total, but never goes quiet for a
    // whole budget's worth. Without milestones resetting it, this exceeds the
    // deadline around the fourth poll and is reported as a failed start.
    let status = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "systemd_user",
        InstalledServiceAction::Restart,
        std::time::Duration::from_millis(400),
        std::time::Duration::from_millis(100),
        || Ok(()),
        |_| {
            probe_calls.set(probe_calls.get() + 1);
            match probe_calls.get() {
                // Still starting, but reporting a distinct milestone each poll.
                1..=8 => {
                    lifecycle.record_startup_progress(
                        port,
                        &format!("warming-project-cache-{}", probe_calls.get()),
                    );
                    DaemonReachability::Unreachable("still starting".to_string())
                }
                _ => DaemonReachability::Reachable,
            }
        },
    )
    .expect("a daemon still reporting startup progress must not be called failed");

    assert!(status.daemon_healthy);
    assert_eq!(status.worker_state, "idle");
    assert!(
        probe_calls.get() > 8,
        "readiness gave up before the daemon became reachable"
    );

    let _ = fs::remove_dir_all(&temp_root);
}

// The converse: silence past the budget is still a stall, so a genuinely hung
// startup is not waited on forever.
#[test]
fn managed_readiness_still_gives_up_when_startup_reports_nothing() {
    let temp_root = unique_temp_dir("cli-system-managed-silent");
    let runtime_root = temp_root.join("run");
    let port = 4562;
    let lifecycle = ready_lifecycle(&runtime_root, port);

    let error = run_service_managed_daemon_with(
        &lifecycle,
        port,
        "systemd_user",
        InstalledServiceAction::Restart,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        || Ok(()),
        |_| DaemonReachability::Unreachable("silent".to_string()),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("did not become reachable"),
        "{error}"
    );

    let _ = fs::remove_dir_all(&temp_root);
}
