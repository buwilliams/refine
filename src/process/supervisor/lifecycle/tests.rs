use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn cargo_launch_reports_and_reuses_the_actual_debug_executable() {
    let actual = PathBuf::from("/checkout/target/debug/refine");
    assert_eq!(
        select_launch_executable("cargo", Some("cargo".to_string()), Some(actual.clone())),
        Some(actual.display().to_string())
    );
    assert_eq!(
        select_launch_executable(
            "cargo",
            Some("/foreign/bin/refine".to_string()),
            Some(actual.clone())
        ),
        Some(actual.display().to_string())
    );
    assert_eq!(
        select_launch_executable(
            "binary",
            Some("/checkout/bin/refine".to_string()),
            Some(actual.clone())
        ),
        Some("/checkout/bin/refine".to_string())
    );
}

#[test]
fn file_lifecycle_persists_port_scoped_status() {
    let temp_root = unique_temp_dir("lifecycle-status");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let service = FileDaemonLifecycleService::new(runtime_root);

    let started = service.recover(4555).unwrap();
    assert!(started.daemon_healthy);
    assert!(service.status_path(4555).exists());
    assert_eq!(service.status(4555).unwrap().worker_state, "idle");

    let stopped = service.stop_runtime(4555).unwrap();
    assert!(!stopped.daemon_healthy);
    assert_eq!(service.status(4555).unwrap().worker_state, "stopped");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn startup_status_is_not_healthy_until_explicitly_ready() {
    let temp_root = unique_temp_dir("lifecycle-startup-status");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let service = FileDaemonLifecycleService::new(runtime_root);

    let starting = service.prepare_start(4555).unwrap();
    assert!(!starting.daemon_healthy);
    assert!(!starting.web_available);
    assert_eq!(starting.worker_state, "starting");
    assert!(!service.status(4555).unwrap().daemon_healthy);

    let ready = service.mark_ready(starting).unwrap();
    assert!(ready.daemon_healthy);
    assert!(ready.web_available);
    assert_eq!(ready.worker_state, "idle");

    let failed = service
        .mark_start_failed(
            4555,
            &RefineError::Io("startup recovery failed".to_string()),
        )
        .unwrap();
    assert!(!failed.daemon_healthy);
    assert!(!failed.web_available);
    assert_eq!(failed.worker_state, "failed");
    assert!(
        failed
            .degraded_integrations
            .iter()
            .any(|entry| entry.contains("startup recovery failed"))
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn observed_readiness_recovers_stale_startup_state_without_hiding_other_evidence() {
    let temp_root = unique_temp_dir("lifecycle-observed-readiness");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let service = FileDaemonLifecycleService::new(runtime_root);
    let mut stale = service
        .mark_start_failed(
            4555,
            &RefineError::Io("failure from an earlier start".to_string()),
        )
        .unwrap();
    stale
        .degraded_integrations
        .push("unrelated-degradation".to_string());
    service.write_status(&stale).unwrap();

    let ready = service.mark_observed_ready(4555).unwrap();

    assert!(ready.daemon_healthy);
    assert!(ready.web_available);
    assert_eq!(ready.worker_state, "idle");
    assert_eq!(
        ready.degraded_integrations,
        vec!["unrelated-degradation".to_string()]
    );
    assert_eq!(service.status(4555).unwrap(), ready);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn background_daemon_launch_records_are_port_scoped() {
    let temp_root = unique_temp_dir("lifecycle-background");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let service = FileDaemonLifecycleService::new(runtime_root.clone());

    let result = service.start_background_daemon(BackgroundDaemonConfig {
        port: 4555,
        bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        cache_dir: None,
        static_root: None,
    });

    assert!(result.is_err());
    assert!(!runtime_root.root.join("processes").exists());
    assert!(!runtime_root.root.join("security-audit.jsonl").exists());
    assert!(runtime_root.port_root(4555).join("processes").exists());
    assert!(
        !runtime_root
            .port_root(4555)
            .join("security-audit.jsonl")
            .exists()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn detached_daemon_command_uses_executable_directly() {
    let exe = PathBuf::from("/usr/local/bin/refine");
    let (command, args) = detached_command_parts(&exe);
    assert_eq!(command, exe.display().to_string());
    assert!(args.is_empty());
}

#[test]
fn file_lifecycle_recover_reconciles_port_scoped_processes() {
    let temp_root = unique_temp_dir("lifecycle-recover");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let supervisor = FileProcessSupervisor::new(runtime_root.port_root(4555));
    supervisor
        .register(crate::process::subprocess::ManagedProcess {
            id: "missing-pid".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("agent".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let operation_registry = FileOperationRegistry::new(runtime_root.port_root(4555));
    let operation = operation_registry
        .register("goal GOAL1 implementation")
        .unwrap();
    let service = FileDaemonLifecycleService::new(runtime_root);

    let status = service.recover(4555).unwrap();
    assert!(status.daemon_healthy);
    assert!(
        status
            .degraded_integrations
            .contains(&"process-recovery-reconciled".to_string())
    );
    assert!(supervisor.inspect("missing-pid").is_err());
    assert!(
        status
            .degraded_integrations
            .contains(&"operation-recovery-interrupted".to_string())
    );
    assert_eq!(
        operation_registry
            .status(&operation.id)
            .unwrap()
            .state
            .as_api_status(),
        "interrupted"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

// A fixed wall-clock deadline measures the host, not the work: a daemon that
// starts comfortably inside the budget on a workstation exceeds it on a loaded
// two-core node and is then recovered as though it had failed. Readiness is
// therefore decided by absence of progress.
#[test]
fn readiness_survives_a_slow_start_that_keeps_making_progress() {
    let start = Instant::now();
    let stall_timeout = Duration::from_secs(120);
    let mut progress = ReadinessProgress::new(stall_timeout, start);

    // Far past any total budget, but still reporting progress each minute.
    let mut now = start;
    for _ in 0..10 {
        now += Duration::from_secs(60);
        assert!(
            !progress.stalled(now),
            "a daemon still producing startup output must not be called failed"
        );
        progress.record_progress(now);
    }

    // Silence past the stall budget is what a genuine stall looks like.
    assert!(!progress.stalled(now + stall_timeout - Duration::from_millis(1)));
    assert!(progress.stalled(now + stall_timeout));
}

#[test]
fn readiness_stalls_when_startup_never_reports_progress() {
    let start = Instant::now();
    let stall_timeout = Duration::from_secs(120);
    let progress = ReadinessProgress::new(stall_timeout, start);

    assert!(!progress.stalled(start + Duration::from_secs(119)));
    assert!(progress.stalled(start + stall_timeout));
}
