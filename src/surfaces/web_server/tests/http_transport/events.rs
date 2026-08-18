use super::*;

#[test]
fn concurrent_sse_clients_share_one_authoritative_frame_build() {
    let daemon = LocalHttpDaemon::new(server_with_projection(), None);
    assert!(Arc::ptr_eq(&daemon.server, &daemon.clone().server));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let mut first = daemon.subscribe_sse_frame_batches("events");
        let mut second = daemon.subscribe_sse_frame_batches("events");
        let first_batch = tokio::time::timeout(Duration::from_secs(2), first.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let second_batch = tokio::time::timeout(Duration::from_secs(2), second.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert!(Arc::ptr_eq(&first_batch, &second_batch));
        assert_eq!(daemon.sse_frame_build_count(), 1);
    });
}

#[test]
fn sse_exposes_typed_state_sync_health() {
    let temp_root = unique_temp_dir("http-sse-state-sync-health");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(temp_root.join(".refine")).unwrap();
    crate::application::persistence_sync::health::FileStateSyncHealthService::new(&runtime_root)
        .record_failure(&temp_root, "default", "git fetch failed")
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);
    let events = LocalHttpDaemon::new(server, None)
        .server_sent_events("events")
        .unwrap();
    assert!(events.contains("event: state_sync_health"), "{events}");
    assert!(events.contains("\"status\":\"failed\""), "{events}");
    assert!(
        events.contains("\"aggregate_counts_authoritative\":false"),
        "{events}"
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn sse_rebuilds_when_state_sync_crosses_the_wall_clock_stale_boundary() {
    let temp_root = unique_temp_dir("http-sse-state-sync-stale-boundary");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    FileSettingsService::with_active_root(&refine_dir, &runtime_root)
        .update(&json!({"state_sync_stale_threshold_seconds": 1}))
        .unwrap();
    let health_service =
        crate::application::persistence_sync::health::FileStateSyncHealthService::new(
            &runtime_root,
        );
    health_service
        .record_success(&temp_root, "default")
        .unwrap();
    let mut record: crate::application::persistence_sync::health::StateSyncHealthRecord =
        serde_json::from_slice(&fs::read(health_service.path()).unwrap()).unwrap();
    let future_success = (Utc::now() + chrono::Duration::seconds(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    record.monitoring_since = future_success.clone();
    record.last_attempt_at = Some(future_success.clone());
    record.last_success_at = Some(future_success);
    fs::write(
        health_service.path(),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);
    let daemon = LocalHttpDaemon::new(server, None);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let mut batches = daemon.subscribe_sse_frame_batches("events");
        tokio::time::timeout(Duration::from_secs(2), batches.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(daemon.sse_frame_build_count(), 1);

        tokio::time::timeout(Duration::from_secs(4), batches.recv())
            .await
            .expect("stale-threshold crossing must invalidate the SSE state")
            .unwrap()
            .unwrap();
        assert_eq!(daemon.sse_frame_build_count(), 2);
        let events = daemon.server_sent_events("events").unwrap();
        assert!(events.contains("\"status\":\"stale\""), "{events}");
    });

    remove_temp_dir(&temp_root);
}

#[test]
fn idle_sse_reuses_the_last_batch_until_an_input_changes() {
    let temp_root = unique_temp_dir("http-sse-idle-batch");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(temp_root.join(".refine")).unwrap();
    let stdout_path = runtime_root.join("idle-sse.stdout.log");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(&stdout_path, "initial\n").unwrap();
    FileProcessSupervisor::new(&runtime_root)
        .register(ManagedProcess {
            id: "idle-sse-process".to_string(),
            owner: ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("idle SSE process".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon::new(server, None);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let mut first = daemon.subscribe_sse_frame_batches("events");
        let initial = tokio::time::timeout(Duration::from_secs(2), first.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(daemon.sse_frame_build_count(), 1);

        FileProcessSupervisor::new(&runtime_root)
            .register(ManagedProcess {
                id: "internal-repository-reconcile".to_string(),
                owner: ProcessOwner::Maintenance,
                pid: Some(std::process::id()),
                state: "running".to_string(),
                label: Some("git".to_string()),
                details: Some(
                    json!({"kind": "repository_reconcile", "command": "git ls-remote"}).to_string(),
                ),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(
            daemon.sse_frame_build_count(),
            1,
            "internal repository commands must not invalidate the public SSE projection"
        );

        let mut reconnect = daemon.subscribe_sse_frame_batches("events");
        let replay = tokio::time::timeout(Duration::from_secs(1), reconnect.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&initial, &replay));
        assert_eq!(daemon.sse_frame_build_count(), 1);

        fs::write(runtime_root.join(API_EVENTS_FILE), "changed\n").unwrap();
        let _changed = tokio::time::timeout(Duration::from_secs(2), reconnect.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(daemon.sse_frame_build_count(), 2);

        fs::write(&stdout_path, "initial\nlater output\n").unwrap();
        let _output_changed = tokio::time::timeout(Duration::from_secs(2), reconnect.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(daemon.sse_frame_build_count(), 3);
    });

    remove_temp_dir(&temp_root);
}

#[test]
fn local_http_daemon_persists_successful_mutations_for_sse() {
    let temp_root = unique_temp_dir("http-mutation-sse");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon::new(server, None);

    let create = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        headers: BTreeMap::new(),
        body: Some(br#"{"id":"GOAL1","name":"SSE Goal"}"#.to_vec()),
    });
    assert_eq!(create.status, 201);
    assert!(runtime_root.join(API_EVENTS_FILE).exists());

    let sse = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/api/sse".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(sse.status, 200);
    let body = String::from_utf8(sse.body).unwrap();
    assert!(body.contains("event: api_mutation"));
    assert!(body.contains("\"path\":\"/work/goals\""));

    remove_temp_dir(&temp_root);
}

#[test]
fn local_http_daemon_keeps_sse_stream_open_over_tcp() {
    let daemon = LocalHttpDaemon::new(server_with_projection(), None);
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let _handle = thread::spawn(move || daemon.serve_once(listener).unwrap());

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();
    stream
        .write_all(b"GET /api/sse HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();

    let mut response = String::new();
    let mut chunk = [0_u8; 512];
    while !response.contains("event: status_change") {
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => panic!("unexpected SSE stream read error: {error}"),
        };
        assert_ne!(read, 0, "SSE stream closed during initial event replay");
        response.push_str(std::str::from_utf8(&chunk[..read]).unwrap());
    }

    let idle_read = loop {
        match stream.read(&mut chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => break result,
        }
    };
    match idle_read {
        Ok(0) => panic!("SSE stream closed after initial event replay"),
        Ok(_) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => {}
        Err(error) => panic!("unexpected SSE stream read error: {error}"),
    }

    let response_lower = response.to_ascii_lowercase();
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response_lower.contains("content-type: text/event-stream"));
    assert!(response.contains("event: ready"));
}

#[test]
fn local_http_daemon_keeps_sse_open_when_process_is_reaped_between_enumeration_and_output() {
    let temp_root = unique_temp_dir("http-sse-process-reaping-race");
    let runtime_root = temp_root.join("run/8080");
    let release_path = temp_root.join("release-process");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'before-reap\\n'; while [ ! -f \"$1\" ]; do sleep 0.01; done".to_string(),
                "process-reaping-race".to_string(),
                release_path.display().to_string(),
            ],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    let stdout_path = PathBuf::from(process.stdout_path.as_deref().unwrap());
    let process_path = supervisor
        .processes_dir()
        .join(format!("{}.json", process.id));

    let (enumerated_tx, enumerated_rx) = std::sync::mpsc::channel();
    let (continue_tx, continue_rx) = std::sync::mpsc::channel();
    install_after_process_enumeration_hook(&runtime_root, move || {
        enumerated_tx.send(()).unwrap();
        continue_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    });

    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon::new(server, None);
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let handle = thread::spawn(move || daemon.serve_once(listener).unwrap());

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();
    stream
        .write_all(b"GET /api/sse HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    enumerated_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    fs::write(&release_path, "").unwrap();
    let reaping_deadline = Instant::now() + Duration::from_secs(2);
    while process_path.exists() && Instant::now() < reaping_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_path.exists(),
        "managed process record was not reaped"
    );
    assert!(
        stdout_path.exists(),
        "terminal process output should remain available for recovery"
    );
    assert!(
        supervisor
            .process_history_dir()
            .join(format!("{}.json", process.id))
            .exists(),
        "terminal process state should be archived"
    );
    continue_tx.send(()).unwrap();

    let mut response = String::new();
    let mut chunk = [0_u8; 1024];
    let initial_deadline = Instant::now() + Duration::from_secs(3);
    while !response.contains("event: status_change") {
        match stream.read(&mut chunk) {
            Ok(0) => panic!("SSE stream reached premature EOF after benign process reaping"),
            Ok(read) => response.push_str(&String::from_utf8_lossy(&chunk[..read])),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) && Instant::now() < initial_deadline => {}
            Err(error) => panic!("failed to read initial SSE events: {error}; {response}"),
        }
    }
    assert!(
        !response.contains("event: error"),
        "benign process reaping emitted the reserved SSE error event: {response}"
    );

    fs::write(
        runtime_root.join("source-promotion.json"),
        serde_json::to_vec(&json!({"id": "after-process-reap"})).unwrap(),
    )
    .unwrap();
    let later_event_deadline = Instant::now() + Duration::from_secs(3);
    while !response.contains("\"id\":\"after-process-reap\"") {
        match stream.read(&mut chunk) {
            Ok(0) => panic!("SSE stream reached EOF before a later event arrived"),
            Ok(read) => response.push_str(&String::from_utf8_lossy(&chunk[..read])),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) && Instant::now() < later_event_deadline => {}
            Err(error) => panic!("failed to read later SSE event: {error}; {response}"),
        }
    }
    assert!(
        !response.contains("event: error"),
        "reserved SSE error appeared before later delivery: {response}"
    );
    assert!(response.contains("event: source_update"));
    assert!(response.contains("\"id\":\"after-process-reap\""));

    drop(stream);
    handle.join().unwrap();
    remove_temp_dir(&temp_root);
}
