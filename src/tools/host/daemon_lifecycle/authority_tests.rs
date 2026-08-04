use super::*;
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::lifecycle::{DaemonLifecycleEvidence, DaemonRuntimeService};
use crate::tools::host::installation::{InstallTarget, InstallationService};

#[test]
fn shared_authority_uses_exact_port_installation_and_isolates_failure_evidence() {
    let temp_root = unique_temp_dir("shared-daemon-lifecycle-two-ports");
    let runtime_root = temp_root.join("run");
    let path_inputs = test_path_inputs(&temp_root);
    let first_port = 4557;
    let second_port = 4558;
    let authority = FileHostDaemonLifecycleService::with_path_inputs(
        RuntimeRoot {
            root: runtime_root.clone(),
        },
        "1.0.0",
        path_inputs,
    );
    for port in [first_port, second_port] {
        authority
            .installation(port)
            .install(InstallTarget::LinuxCliWeb)
            .unwrap();
    }

    let first_backend = authority
        .installation(first_port)
        .status()
        .unwrap()
        .backend
        .unwrap();
    let second_backend = authority
        .installation(second_port)
        .status()
        .unwrap()
        .backend
        .unwrap();
    assert!(
        first_backend
            .service_metadata_path
            .as_deref()
            .unwrap()
            .ends_with("refine-4557.service")
    );
    assert!(
        second_backend
            .service_metadata_path
            .as_deref()
            .unwrap()
            .ends_with("refine-4558.service")
    );
    assert_ne!(
        first_backend.service_metadata_path,
        second_backend.service_metadata_path
    );

    let error = authority
        .exercise_with(
            BackgroundDaemonConfig {
                port: first_port,
                ..Default::default()
            },
            InstalledServiceAction::Restart,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            |installation, action| {
                assert_eq!(installation.port, Some(first_port));
                assert_eq!(action, InstalledServiceAction::Restart);
                Err(RefineError::Degraded(
                    "systemctl restart failed for first port".to_string(),
                ))
            },
            |_| DaemonReachability::Unreachable("connection refused".to_string()),
        )
        .unwrap_err();
    assert_eq!(error.to_string(), "systemctl restart failed for first port");

    let first_status = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    })
    .status(first_port)
    .unwrap();
    assert_eq!(first_status.lifecycle_evidence.unwrap().action, "restart");
    assert!(
        !FileDaemonLifecycleService::new(RuntimeRoot {
            root: runtime_root.clone(),
        })
        .status_path(second_port)
        .exists(),
        "the other port must not receive lifecycle evidence"
    );

    assert_eq!(
        authority
            .installation(4559)
            .installed_service_manager()
            .unwrap(),
        None,
        "an uninstalled port selects the direct-process fallback"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_executor_routes_every_surface_action_without_changing_status_or_evidence() {
    #[derive(Default)]
    struct RecordingLifecycle {
        actions: std::cell::RefCell<Vec<&'static str>>,
    }

    impl HostDaemonLifecycleService for RecordingLifecycle {
        fn start(&self, config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus> {
            self.actions.borrow_mut().push("start");
            Ok(recorded_status(config.port, "start"))
        }

        fn stop(&self, port: u16) -> RefineResult<DaemonStatus> {
            self.actions.borrow_mut().push("stop");
            Ok(recorded_status(port, "stop"))
        }

        fn restart(&self, config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus> {
            self.actions.borrow_mut().push("restart");
            Ok(recorded_status(config.port, "restart"))
        }
    }

    let lifecycle = RecordingLifecycle::default();
    for (action, expected) in [
        (DaemonLifecycleAction::Start, "start"),
        (DaemonLifecycleAction::Stop, "stop"),
        (DaemonLifecycleAction::Restart, "restart"),
    ] {
        let status = execute_daemon_lifecycle(
            &lifecycle,
            action,
            BackgroundDaemonConfig {
                port: 4557,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(status.worker_state, expected);
        assert_eq!(status.lifecycle_evidence.as_ref().unwrap().action, expected);
    }
    assert_eq!(
        lifecycle.actions.into_inner(),
        vec!["start", "stop", "restart"]
    );
}

#[test]
fn shared_authority_uses_direct_fallback_for_an_uninstalled_reachable_port() {
    let temp_root = unique_temp_dir("shared-daemon-lifecycle-direct-fallback");
    let runtime_root = temp_root.join("run");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let responder = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });
    let authority = FileHostDaemonLifecycleService::with_path_inputs(
        RuntimeRoot {
            root: runtime_root.clone(),
        },
        "1.0.0",
        test_path_inputs(&temp_root),
    );

    let started = authority
        .start(BackgroundDaemonConfig {
            port,
            ..Default::default()
        })
        .unwrap();
    responder.join().unwrap();
    assert!(started.daemon_healthy);
    assert_eq!(started.worker_state, "idle");
    assert_eq!(started.lifecycle_evidence, None);

    let stopped = authority.stop(port).unwrap();
    assert!(!stopped.daemon_healthy);
    assert_eq!(stopped.worker_state, "stopped");
    assert_eq!(
        stopped.lifecycle_evidence.unwrap().observed_reachable,
        Some(false)
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn uninstalling_registered_but_inactive_installation_stops_observed_direct_runtime() {
    let temp_root = unique_temp_dir("shared-daemon-lifecycle-inactive-registration");
    let runtime_root = temp_root.join("run");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let authority = FileHostDaemonLifecycleService::with_path_inputs(
        RuntimeRoot {
            root: runtime_root.clone(),
        },
        "1.0.0",
        test_path_inputs(&temp_root),
    );
    let installation = authority.installation(port);
    installation.install(InstallTarget::LinuxCliWeb).unwrap();
    let mut backend: crate::tools::host::installation::InstallBackendRegistration =
        serde_json::from_slice(&fs::read(installation.backend_path()).unwrap()).unwrap();
    backend.activated = false;
    backend.activation_error = Some("systemctl --user unavailable".to_string());
    fs::write(
        installation.backend_path(),
        serde_json::to_vec_pretty(&backend).unwrap(),
    )
    .unwrap();

    let runtime = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    });
    let supervisor = FileProcessSupervisor::new(runtime_root.join(port.to_string()));
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Daemon,
            command: std::env::current_exe().unwrap().display().to_string(),
            args: vec![
                "tools::host::daemon_lifecycle::authority_tests::direct_runtime_listener_child"
                    .to_string(),
                "--exact".to_string(),
                "--ignored".to_string(),
                "--nocapture".to_string(),
            ],
            cwd: None,
            env: vec![(
                "REFINE_DIRECT_RUNTIME_TEST_PORT".to_string(),
                port.to_string(),
            )],
            stdin: None,
            limits: None,
            authorization_command: Some("direct-runtime-listener-test".to_string()),
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while http_reachability_probe(port) != DaemonReachability::Reachable {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for direct runtime listener"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let recovered = runtime.recover(port).unwrap();
    runtime.mark_ready(recovered).unwrap();

    uninstall_daemon_installation(&authority, &installation, port).unwrap();
    let stopped = runtime.status(port).unwrap();
    assert_eq!(stopped.worker_state, "stopped");
    assert!(!stopped.daemon_healthy);
    let evidence = stopped.lifecycle_evidence.unwrap();
    assert_eq!(evidence.service_manager, "direct_process");
    assert_eq!(evidence.observed_reachable, Some(false));
    assert_ne!(supervisor.wait(&process.id).unwrap().state, "running");
    assert!(!installation.status().unwrap().installed);
    assert!(!installation.backend_path().exists());
    assert_eq!(
        installation
            .installed_service_manager_for(InstalledServiceAction::Stop)
            .unwrap(),
        None
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn failed_uninstall_shutdown_preserves_service_registration() {
    struct FailingStop;

    impl HostDaemonLifecycleService for FailingStop {
        fn start(&self, _config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus> {
            unreachable!()
        }

        fn stop(&self, _port: u16) -> RefineResult<DaemonStatus> {
            Err(RefineError::Degraded(
                "daemon remained reachable after service stop".to_string(),
            ))
        }

        fn restart(&self, _config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus> {
            unreachable!()
        }
    }

    let temp_root = unique_temp_dir("failed-uninstall-preserves-registration");
    let runtime_root = temp_root.join("run");
    let authority = FileHostDaemonLifecycleService::with_path_inputs(
        RuntimeRoot {
            root: runtime_root.clone(),
        },
        "1.0.0",
        test_path_inputs(&temp_root),
    );
    let installation = authority.installation(4557);
    let installed = installation.install(InstallTarget::LinuxCliWeb).unwrap();
    let metadata = PathBuf::from(
        installed
            .backend
            .as_ref()
            .and_then(|backend| backend.service_metadata_path.as_ref())
            .unwrap(),
    );

    let error = uninstall_daemon_installation(&FailingStop, &installation, 4557).unwrap_err();

    assert_eq!(
        error.to_string(),
        "daemon remained reachable after service stop"
    );
    assert!(installation.status().unwrap().installed);
    assert!(installation.backend_path().exists());
    assert!(metadata.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
#[ignore = "helper invoked only by the registered-but-inactive direct-runtime test"]
fn direct_runtime_listener_child() {
    let Some(port) = std::env::var("REFINE_DIRECT_RUNTIME_TEST_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
    else {
        return;
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 81\r\nConnection: close\r\n\r\n{\"product\":\"refine\",\"version\":\"test\",\"executable_path\":\"/tmp/direct-test-refine\"}",
            )
            .unwrap();
    }
}

fn recorded_status(port: u16, action: &str) -> DaemonStatus {
    DaemonStatus {
        port,
        daemon_healthy: action != "stop",
        web_available: action != "stop",
        worker_state: action.to_string(),
        target_app_state: "detached".to_string(),
        launch_mode: "test".to_string(),
        executable_path: None,
        active_operations: Vec::new(),
        degraded_integrations: Vec::new(),
        lifecycle_evidence: Some(DaemonLifecycleEvidence {
            action: action.to_string(),
            service_manager: "test".to_string(),
            outcome: format!("{action}_test"),
            command_error: None,
            readiness_error: None,
            observed_reachable: Some(action != "stop"),
            recovery: None,
        }),
    }
}

fn test_path_inputs(temp_root: &Path) -> RuntimePathInputs {
    RuntimePathInputs {
        home: Some(temp_root.join("home")),
        local_app_data: Some(temp_root.join("local-app-data")),
        app_data: Some(temp_root.join("app-data")),
        program_data: Some(temp_root.join("program-data")),
        xdg_cache_home: Some(temp_root.join("cache")),
        xdg_state_home: Some(temp_root.join("state")),
        xdg_config_home: Some(temp_root.join("config")),
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nonce}", std::process::id()))
}
