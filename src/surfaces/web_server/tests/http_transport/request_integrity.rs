use super::*;

#[test]
fn local_http_daemon_validates_origin_version_and_idempotency_headers() {
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };

    let forbidden = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        headers: BTreeMap::from([
            ("host".to_string(), "refine.internal:8082".to_string()),
            ("origin".to_string(), "https://example.com".to_string()),
        ]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(forbidden.status, 403);
    let forbidden_body: serde_json::Value = serde_json::from_slice(&forbidden.body).unwrap();
    assert_eq!(
        forbidden_body["error"]["message"],
        "mutation request origin must match the request host"
    );

    let same_origin = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([
            (
                "host".to_string(),
                "bo2lnxnevo03.ins.insurity.net:8082".to_string(),
            ),
            (
                "origin".to_string(),
                "http://bo2lnxnevo03.ins.insurity.net:8082".to_string(),
            ),
        ]),
        body: None,
    });
    assert_eq!(same_origin.status, 404);

    let same_origin_referer = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([
            ("host".to_string(), "10.20.30.40:8082".to_string()),
            (
                "referer".to_string(),
                "http://10.20.30.40:8082/settings".to_string(),
            ),
        ]),
        body: None,
    });
    assert_eq!(same_origin_referer.status, 404);

    let same_origin_https = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([
            ("host".to_string(), "refine.example.com".to_string()),
            (
                "origin".to_string(),
                "https://refine.example.com".to_string(),
            ),
        ]),
        body: None,
    });
    assert_eq!(same_origin_https.status, 404);

    let wrong_port = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([
            ("host".to_string(), "refine.example.com:8082".to_string()),
            (
                "origin".to_string(),
                "http://refine.example.com:8083".to_string(),
            ),
        ]),
        body: None,
    });
    assert_eq!(wrong_port.status, 403);

    let tauri = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([("origin".to_string(), "tauri://localhost".to_string())]),
        body: None,
    });
    assert_eq!(tauri.status, 404);

    let tauri_https = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([("origin".to_string(), "https://tauri.localhost/".to_string())]),
        body: None,
    });
    assert_eq!(tauri_https.status, 404);

    let no_origin = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(no_origin.status, 404);

    let opaque_origin = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([
            ("host".to_string(), "refine.example.com:8082".to_string()),
            ("origin".to_string(), "null".to_string()),
        ]),
        body: None,
    });
    assert_eq!(opaque_origin.status, 403);

    let missing_host = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([(
            "origin".to_string(),
            "http://refine.example.com:8082".to_string(),
        )]),
        body: None,
    });
    assert_eq!(missing_host.status, 403);

    let loopback_origin_foreign_host = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/missing".to_string(),
        headers: BTreeMap::from([
            ("host".to_string(), "attacker.example.com".to_string()),
            ("origin".to_string(), "http://localhost:8082".to_string()),
        ]),
        body: None,
    });
    assert_eq!(loopback_origin_foreign_host.status, 403);

    let version = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        headers: BTreeMap::from([("x-refine-api-version".to_string(), "999".to_string())]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(version.status, 426);

    let idempotency = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        headers: BTreeMap::from([("idempotency-key".to_string(), "bad key".to_string())]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(idempotency.status, 400);
}

#[test]
fn local_http_daemon_replays_idempotent_mutation_responses() {
    let temp_root = unique_temp_dir("http-idempotency");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let body = br#"{"id":"GOAL1","name":"Idempotent Goal"}"#.to_vec();
    let headers = BTreeMap::from([("idempotency-key".to_string(), "create-goal-1".to_string())]);

    let first = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        headers: headers.clone(),
        body: Some(body.clone()),
    });
    assert_eq!(first.status, 201);
    let second = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        headers: headers.clone(),
        body: Some(body),
    });
    assert_eq!(second.status, 201);
    assert_eq!(first.body, second.body);
    assert_eq!(
        fs::read_dir(refine_dir.join("goals/GO/AL1"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "goal.json")
            .count(),
        1
    );
    assert!(
        runtime_root
            .join(IDEMPOTENCY_DIR)
            .join("create-goal-1.json")
            .exists()
    );
    let cached_projection: ProjectionSnapshot = serde_json::from_str(
        &fs::read_to_string(runtime_root.join("cache").join(PROJECTION_SNAPSHOT_FILE)).unwrap(),
    )
    .unwrap();
    assert!(cached_projection.goals.contains_key("GOAL1"));

    remove_temp_dir(&temp_root);
}

#[test]
fn local_http_daemon_rejects_idempotency_key_reuse_for_different_requests() {
    let temp_root = unique_temp_dir("http-idempotency-conflict");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root);
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let headers = BTreeMap::from([(
        "idempotency-key".to_string(),
        "create-goal-conflict".to_string(),
    )]);

    let first = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        headers: headers.clone(),
        body: Some(br#"{"id":"GOAL1","name":"First"}"#.to_vec()),
    });
    assert_eq!(first.status, 201);
    let conflict = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        headers,
        body: Some(br#"{"id":"GOAL2","name":"Second"}"#.to_vec()),
    });
    assert_eq!(conflict.status, 409);
    let body: serde_json::Value = serde_json::from_slice(&conflict.body).unwrap();
    assert_eq!(body["error"]["code"], "idempotency_conflict");

    remove_temp_dir(&temp_root);
}
