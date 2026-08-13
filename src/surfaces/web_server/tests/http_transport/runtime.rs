use super::*;

#[test]
fn local_http_daemon_serves_projection_routes_over_tcp() {
    let daemon = LocalHttpDaemon::new(server_with_projection(), None);
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let handle = thread::spawn(move || daemon.serve_once(listener).unwrap());

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(b"GET /work/goals HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    handle.join().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("\"id\": \"GOAL1\""));
    assert!(response.contains("\"counts\""));
}

#[test]
fn local_http_daemon_handles_tcp_requests_on_worker_threads() {
    let daemon = LocalHttpDaemon::new(server_with_projection(), None);
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let handle = thread::spawn(move || daemon.serve_once(listener).unwrap());

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(b"GET /system/version HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    handle.join().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("\"product\": \"refine\""));
}

#[test]
fn local_http_daemon_stays_responsive_while_plan_start_waits_for_git() {
    let temp_root = unique_temp_dir("http-plan-git-wait");
    let app_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&app_root);
    fs::create_dir_all(refine_dir_for_target_root(&app_root).unwrap()).unwrap();

    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let lock_root = app_root.clone();
    let lock_thread = thread::spawn(move || {
        crate::tools::host::git_sync::with_repository_git_lock(&lock_root, || {
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    locked_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root);
    server.runtime_root = Some(runtime_root);
    let daemon = LocalHttpDaemon::new(server, None);
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let server_thread = thread::spawn(move || daemon.serve_once(listener).unwrap());

    let (sent_tx, sent_rx) = std::sync::mpsc::channel();
    let blocked_request = thread::spawn(move || {
        let body = r#"{"purpose":"plan"}"#;
        let mut stream = TcpStream::connect(addr).unwrap();
        let request = format!(
            "POST /api/chat/start HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        sent_tx.send(()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });
    sent_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    thread::sleep(Duration::from_millis(50));

    let mut responsive = TcpStream::connect(addr).unwrap();
    responsive
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    responsive
        .write_all(b"GET /system/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    responsive.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    release_tx.send(()).unwrap();
    lock_thread.join().unwrap();
    let plan_response = blocked_request.join().unwrap();
    assert!(plan_response.starts_with("HTTP/1.1 201 Created"));
    server_thread.join().unwrap();
    remove_temp_dir(&temp_root);
}

#[test]
fn local_http_daemon_reports_startup_cache_progress() {
    let daemon = LocalHttpDaemon::new(server_with_projection(), None);
    let mut messages = Vec::new();

    daemon
        .recover_runtime_state_with_progress(|message| messages.push(message.to_string()))
        .unwrap();

    assert_eq!(
        messages,
        vec![
            "warming project and runtime caches",
            "warming diagnostics cache",
            "warming static asset cache",
            "startup cache warming complete",
        ]
    );
}

#[test]
fn local_http_daemon_refreshes_hot_projection_and_records_screen_metrics() {
    let temp_root = unique_temp_dir("http-hot-projection-metrics");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon::new(server, None);
    daemon.recover_runtime_state().unwrap();

    let create = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        headers: BTreeMap::new(),
        body: Some(br#"{"id":"HOT1","name":"Hot cached Goal"}"#.to_vec()),
    });
    assert_eq!(create.status, 201);

    let list = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/api/goals?limit=50&offset=0".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(list.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&list.body).unwrap();
    assert_eq!(body["goals"][0]["id"], "HOT1");

    let events = wait_for_http_request_metrics(&runtime_root);
    assert!(events.iter().any(|event| {
        event.operation == "http.request"
            && event.details.get("method").and_then(|value| value.as_str()) == Some("POST")
            && event
                .details
                .get("budget_ms")
                .and_then(|value| value.as_f64())
                == Some(50.0)
    }));
    assert!(events.iter().any(|event| {
        event.operation == "http.request"
            && event.details.get("path").and_then(|value| value.as_str()) == Some("/work/goals")
    }));

    for path in [
        "/api/dashboard?node=current",
        "/api/goals?limit=50&offset=0",
        "/api/features?limit=50&offset=0",
        "/api/activity?limit=50&offset=0",
        "/api/changes?limit=50&offset=0",
        "/api/nodes",
        "/api/settings",
        "/api/processes",
        "/api/diagnostics",
        "/api/performance?limit=50&offset=0",
    ] {
        let started = Instant::now();
        let response = daemon.handle_wire_request(HttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: BTreeMap::new(),
            body: None,
        });
        let elapsed = started.elapsed();
        assert_eq!(response.status, 200, "{path}");
        // Keep enough headroom for the repository's heavily parallel unit suite;
        // request-level performance budgets are recorded separately in metrics.
        assert!(
            elapsed < Duration::from_millis(500),
            "{path} took {:?}",
            elapsed
        );
    }

    let events = wait_for_http_request_metric_count(&runtime_root, 10);
    assert!(events.len() >= 10);

    remove_temp_dir(&temp_root);
}

#[test]
fn local_http_daemon_recovers_stale_chat_turns_before_serving() {
    let temp_root = unique_temp_dir("http-chat-recovery");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
    let session = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    let operation = FileOperationRegistry::new(&runtime_root)
        .register(&format!("chat:{}", session.id))
        .unwrap();
    let session_path = refine_dir.join(format!("chat/sessions/{}.json", session.id));

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon::new(server, None);
    daemon.recover_runtime_state().unwrap();

    let recovered: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    assert!(recovered.get("in_flight").is_none());
    assert!(recovered.get("last_turn_started_at").is_none());
    assert_eq!(recovered["interrupted"], true);
    assert!(
        recovered["interruption_detail"]
            .as_str()
            .unwrap_or("")
            .contains("Daemon restarted")
    );
    assert_eq!(
        FileOperationRegistry::new(&runtime_root)
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Interrupted
    );

    remove_temp_dir(&temp_root);
}
