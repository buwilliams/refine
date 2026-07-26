use super::*;

#[test]
fn system_start_resolves_relative_runtime_root_before_spawning_daemon() {
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        absolute_cli_path(PathBuf::from("run")).unwrap(),
        cwd.join("run")
    );
    assert_eq!(
        absolute_cli_path(cwd.join("already-absolute")).unwrap(),
        cwd.join("already-absolute")
    );
}

#[test]
fn system_start_owns_foreground_web_options() {
    let parsed = Cli::try_parse_from([
        "refine",
        "system",
        "start",
        "--port",
        "0",
        "--runtime-root",
        "run",
        "--once",
    ])
    .unwrap();
    let Commands::System {
        action:
            SystemAction::Start {
                port,
                bind_address,
                runtime_root,
                once,
                foreground,
                ..
            },
    } = parsed.command
    else {
        panic!("expected system start command");
    };
    assert_eq!(port, 0);
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(runtime_root, PathBuf::from("run"));
    assert!(once);
    assert!(!foreground);

    let parsed = Cli::try_parse_from([
        "refine",
        "system",
        "start",
        "--bind-address",
        "0.0.0.0",
        "--once",
    ])
    .unwrap();
    let Commands::System {
        action: SystemAction::Start { bind_address, .. },
    } = parsed.command
    else {
        panic!("expected system start command");
    };
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    assert!(Cli::try_parse_from(["refine", "system", "web"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "web", "--target-root", ".refine"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "serve", "--once"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "start", "--token", "secret"]).is_err());
}

#[test]
fn system_lifecycle_commands_default_to_8082() {
    for (verb, expected) in [
        ("start", "Start"),
        ("stop", "Stop"),
        ("restart", "Restart"),
        ("status", "Status"),
    ] {
        let parsed = Cli::try_parse_from(["refine", "system", verb]).unwrap();
        let Commands::System { action } = parsed.command else {
            panic!("expected system command");
        };
        let port = match action {
            SystemAction::Start { port, .. }
            | SystemAction::Stop { port, .. }
            | SystemAction::Restart { port, .. }
            | SystemAction::Status { port, .. } => port,
            other => panic!("expected {expected} action, got {other:?}"),
        };
        assert_eq!(port, 8082, "{expected} should default to port 8082");
    }
}

#[test]
fn system_start_migrates_and_detaches_an_unavailable_port_scoped_project() {
    let temp_root = unique_temp_dir("cli-system-start-stale-project");
    let runtime_root = temp_root.join("run");
    let port_probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    fs::create_dir_all(&port_runtime_root).unwrap();
    let missing_project = temp_root.join("missing-project");
    fs::write(
        port_runtime_root.join("apps.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "active_app": missing_project.display().to_string(),
            "apps": {
                (missing_project.display().to_string()): {
                    "name": "missing-project",
                    "path": missing_project.display().to_string(),
                    "added_at": "2026-07-26T00:00:00Z",
                    "last_used_at": "2026-07-26T00:00:00Z"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let start_runtime_root = runtime_root.clone();
    let server = thread::spawn(move || {
        run_system_start(
            port,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
            None,
            start_runtime_root,
            true,
            true,
        )
    });
    let mut stream = connect_to_cli_test_daemon(port);
    stream
        .write_all(b"GET /system/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    server.join().unwrap().unwrap();

    let canonical: serde_json::Value =
        serde_json::from_slice(&fs::read(runtime_root.join("apps.json")).unwrap()).unwrap();
    assert!(canonical["active_app"].is_null());
    assert_eq!(
        canonical["apps"][missing_project.display().to_string()]["path"],
        missing_project.display().to_string()
    );
    let status: crate::process::supervisor::lifecycle::DaemonStatus =
        serde_json::from_slice(&fs::read(port_runtime_root.join("daemon-status.json")).unwrap())
            .unwrap();
    assert!(status.daemon_healthy);
    assert!(
        status
            .degraded_integrations
            .contains(&"active-project-unavailable".to_string())
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_status_reports_current_version_and_running_ports() {
    let temp_root = unique_temp_dir("cli-system-status");
    let runtime_root = temp_root.join("run");
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let live_port = listener.local_addr().unwrap().port();
    let probe_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 512];
        let _ = stream.read(&mut buffer);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
    });
    let stale_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let stale_port = stale_listener.local_addr().unwrap().port();
    drop(stale_listener);

    lifecycle.start(live_port).unwrap();
    lifecycle.start(stale_port).unwrap();
    lifecycle.start(4556).unwrap();
    lifecycle.stop(4556).unwrap();
    FileProcessSupervisor::new(
        RuntimeRoot {
            root: runtime_root.clone(),
        }
        .port_root(live_port),
    )
    .register(ManagedProcess {
        id: "helper-1".to_string(),
        owner: ProcessOwner::UserHelper,
        pid: Some(std::process::id()),
        state: "running".to_string(),
        label: Some("helper".to_string()),
        details: Some("{\"kind\":\"ui\",\"secret\":\"not-for-status\"}".to_string()),
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: String::new(),
        exit_code: None,
    })
    .unwrap();
    fs::create_dir_all(runtime_root.join("not-a-port")).unwrap();

    let status = system_status_response(runtime_root).unwrap();
    probe_thread.join().unwrap();
    assert_eq!(status["product"], "refine");
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["current_version"], env!("CARGO_PKG_VERSION"));
    assert!(status["launch_mode"].is_string());
    assert!(status["executable_path"].is_string());
    assert_eq!(status["running_ports"], serde_json::json!([live_port]));
    assert_eq!(status["ports"].as_array().unwrap().len(), 1);
    assert_eq!(status["ports"][0]["port"], live_port);
    assert!(status["ports"][0]["launch_mode"].is_string());
    assert!(status["ports"][0]["executable_path"].is_string());
    assert!(status["ports"][0]["daemon_healthy"].as_bool().unwrap());
    assert_eq!(status["ports"][0]["process_count"], 1);
    let process = status["ports"][0]["processes"][0].as_object().unwrap();
    assert_eq!(process.len(), 3);
    assert!(process.contains_key("pid"));
    assert!(process.contains_key("status"));
    assert!(process.contains_key("label"));
    assert_eq!(process["pid"], serde_json::json!(std::process::id()));
    assert_eq!(process["status"], "running");
    assert_eq!(process["label"], "helper");
    assert!(status["ports"][0].get("process_summary").is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn cli_goal_lifecycle_membership_and_feature_edit_use_tool_services() {
    let temp_root = unique_temp_dir("cli-goal-lifecycle");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    for (command, args) in [
        (
            "goal",
            vec![
                "create",
                "Lifecycle Goal",
                "--target-root",
                target_root.to_str().unwrap(),
                "--id",
                "GOAL1",
            ],
        ),
        (
            "feature",
            vec![
                "create",
                "Feature One",
                "--target-root",
                target_root.to_str().unwrap(),
                "--id",
                "FEA1",
            ],
        ),
    ] {
        let mut argv = vec!["refine", command];
        argv.extend(args);
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "assign-feature",
            "GOAL1",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"feature_id\": \"FEA1\"")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "edit",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
            "--name",
            "Renamed Feature",
            "--description",
            "Edited",
            "--reporter",
            "QA",
        ])
        .unwrap(),
    )
    .unwrap();
    let feature = fs::read_to_string(refine_dir.join("features/FE/A1/feature.json")).unwrap();
    assert!(feature.contains("\"name\": \"Renamed Feature\""));
    assert!(feature.contains("\"reporter\": \"QA\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "remove-feature",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"feature_id\": null")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "start",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}
