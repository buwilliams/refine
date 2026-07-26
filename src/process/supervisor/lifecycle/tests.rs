use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn file_lifecycle_persists_port_scoped_status() {
    let temp_root = unique_temp_dir("lifecycle-status");
    let runtime_root = RuntimeRoot {
        root: temp_root.join("run"),
    };
    let service = FileDaemonLifecycleService::new(runtime_root);

    let started = service.start(4555).unwrap();
    assert!(started.daemon_healthy);
    assert!(service.status_path(4555).exists());
    assert_eq!(service.status(4555).unwrap().worker_state, "idle");

    let stopped = service.stop(4555).unwrap();
    assert!(!stopped.daemon_healthy);
    assert_eq!(service.status(4555).unwrap().worker_state, "stopped");

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
