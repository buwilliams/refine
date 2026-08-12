use super::*;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::process::supervisor::lifecycle::DaemonLifecycleEvidence;
use crate::process::supervisor::lifecycle::DaemonRuntimeService;

#[derive(Default)]
struct RecordingLauncher {
    launches: RefCell<Vec<(RestartSafeHandoff, Option<String>)>>,
    failure: Option<&'static str>,
}

impl RestartSafeHandoffLauncher for RecordingLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<()> {
        self.launches
            .borrow_mut()
            .push((handoff.clone(), service_manager.map(str::to_string)));
        match self.failure {
            Some(message) => Err(RefineError::Io(message.to_string())),
            None => Ok(()),
        }
    }
}

#[test]
fn source_lifecycle_handoff_inherits_debug_executable_without_installed_binary() {
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let runtime_root = checkout.join("run");
    let test_executable = std::env::current_exe().unwrap();

    let selected =
        operations::lifecycle_executable_for_invocation(&runtime_root, &test_executable).unwrap();

    assert_eq!(selected, test_executable.canonicalize().unwrap());
    assert_ne!(selected, checkout.join("bin/refine"));
}

#[test]
fn http_handoff_persists_receipt_before_launch_and_reconciles_shared_result() {
    let temp_root = unique_temp_dir("daemon-lifecycle-operation-success");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let port = 4557;
    let runtime = FileDaemonLifecycleService::new(runtime_root.clone());
    let recovered = runtime.recover(port).unwrap();
    runtime.mark_ready(recovered).unwrap();

    let service = FileDaemonLifecycleOperationService::new(runtime_root, "1.0.0");
    let launcher = RecordingLauncher::default();
    let queued = service
        .queue_with(
            DaemonLifecycleAction::Stop,
            BackgroundDaemonConfig {
                port,
                ..Default::default()
            },
            Path::new("/mock/refine"),
            None,
            &launcher,
        )
        .unwrap();
    assert_eq!(queued.status, "queued");
    assert_eq!(service.load(port, &queued.id).unwrap(), queued);
    let launches = launcher.launches.borrow();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].1, None);
    assert_eq!(launches[0].0.args[1], "daemon-lifecycle-helper");
    assert_eq!(launches[0].0.args.last(), Some(&queued.id));
    drop(launches);

    let settled = service
        .run_helper(
            &queued.id,
            DaemonLifecycleAction::Stop,
            BackgroundDaemonConfig {
                port,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(settled.status, "succeeded");
    assert_eq!(settled.result.as_ref().unwrap().worker_state, "stopped");
    assert_eq!(service.load(port, &queued.id).unwrap(), settled);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn handoff_launch_failure_is_durable_and_never_attempts_control() {
    let temp_root = unique_temp_dir("daemon-lifecycle-operation-launch-failure");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let service = FileDaemonLifecycleOperationService::new(runtime_root, "1.0.0");
    let launcher = RecordingLauncher {
        failure: Some("transient service submission denied"),
        ..Default::default()
    };
    let error = service
        .queue_with(
            DaemonLifecycleAction::Restart,
            BackgroundDaemonConfig {
                port: 4558,
                ..Default::default()
            },
            Path::new("/mock/refine"),
            Some("systemd_user"),
            &launcher,
        )
        .unwrap_err();
    assert!(error.to_string().contains("submission denied"));

    let operations = fs::read_dir(
        temp_root
            .join("run/4558")
            .join(DAEMON_LIFECYCLE_OPERATIONS_DIR),
    )
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .collect::<Vec<_>>();
    assert_eq!(operations.len(), 1);
    let failed: DaemonLifecycleOperation =
        serde_json::from_slice(&fs::read(&operations[0]).unwrap()).unwrap();
    assert_eq!(failed.status, "failed");
    assert!(failed.result.is_none());
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("submission denied")
    );
    assert!(
        failed
            .recovery
            .as_deref()
            .unwrap()
            .contains("No daemon control")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn lifecycle_control_failure_settles_operation_with_observed_recovery_evidence() {
    let temp_root = unique_temp_dir("daemon-lifecycle-operation-control-failure");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let port = 4559;
    let service = FileDaemonLifecycleOperationService::new(runtime_root, "1.0.0");
    let launcher = RecordingLauncher::default();
    let queued = service
        .queue_with(
            DaemonLifecycleAction::Restart,
            BackgroundDaemonConfig {
                port,
                ..Default::default()
            },
            Path::new("/mock/refine"),
            Some("systemd_user"),
            &launcher,
        )
        .unwrap();

    let error = service
        .run_helper_with(
            &queued.id,
            DaemonLifecycleAction::Restart,
            BackgroundDaemonConfig {
                port,
                ..Default::default()
            },
            std::time::Duration::ZERO,
            |lifecycle, _, _| {
                lifecycle
                    .runtime_lifecycle()
                    .mark_observed_failed_with_evidence(
                        port,
                        "restart-failed:systemctl transport unavailable".to_string(),
                        Some(DaemonLifecycleEvidence {
                            action: "restart".to_string(),
                            service_manager: "systemd_user".to_string(),
                            outcome: "restart_command_failed_daemon_reachability_unknown"
                                .to_string(),
                            command_error: Some("systemctl transport unavailable".to_string()),
                            readiness_error: Some("fresh probe timed out".to_string()),
                            observed_reachable: None,
                            recovery: Some(
                                "inspect systemd and daemon reachability before retrying"
                                    .to_string(),
                            ),
                        }),
                    )?;
                Err(RefineError::Degraded(
                    "systemctl transport unavailable".to_string(),
                ))
            },
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("systemctl transport unavailable")
    );

    let failed = service.load(port, &queued.id).unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed
            .result
            .as_ref()
            .unwrap()
            .lifecycle_evidence
            .as_ref()
            .unwrap()
            .observed_reachable,
        None
    );
    assert!(
        failed
            .recovery
            .as_deref()
            .unwrap()
            .contains("inspect systemd")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nonce}", std::process::id()))
}
