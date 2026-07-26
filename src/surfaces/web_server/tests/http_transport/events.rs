use super::*;

#[test]
fn local_http_daemon_persists_successful_mutations_for_sse() {
    let temp_root = unique_temp_dir("http-mutation-sse");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };

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
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };
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
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
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
        !stdout_path.exists(),
        "managed process output was not reaped"
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
    while !response.contains("event: source_promotion") {
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
    assert!(response.contains("\"id\":\"after-process-reap\""));

    drop(stream);
    handle.join().unwrap();
    remove_temp_dir(&temp_root);
}
