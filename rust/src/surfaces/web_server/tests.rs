use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::metrics::FileMetricsService;
use crate::core::product::chat::{ChatAttachment, ChatService, FileChatService};
use crate::core::product::scheduling::{FileSchedulingService, SchedulingService};
use crate::core::supervisor::jobs::{FileJobRegistry, JobRegistry, JobState};
use crate::model::log::LogEntry;
use serde_json::json;

use crate::core::supervisor::errors::{RefineError, RefineResult};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcess, ProcessSupervisor,
};
use crate::core::product::project_state::{
    DashboardProjection, FeatureSummaryProjection, FileProjectStateStore, GapSummaryProjection,
    PROJECTION_SNAPSHOT_FILE, PROJECTION_SNAPSHOT_VERSION, ProjectStateStore, ProjectionSnapshot,
    RuntimeProjection,
};
use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::gap::{GapIndexProjection, GapPriority};
use crate::model::log::ActivityEntry;
use crate::model::workflow::GapStatus;

#[test]
fn web_server_routes_work_gap_queries_through_projection() {
    let mut server = server_with_projection();
    server.projection.gaps.insert(
        "GAP2".to_string(),
        GapSummaryProjection {
            gap: GapIndexProjection {
                id: "GAP2".to_string(),
                name: "Settings route".to_string(),
                status: GapStatus::Done,
                priority: GapPriority::High,
                reporter: Some("Alice".to_string()),
                round_count: 3,
                created: "created2".to_string(),
                updated: "updated2".to_string(),
                branch_name: None,
                node_id: Some("node-b".to_string()),
                feature_id: Some("FEA1".to_string()),
                feature_order: Some(1),
                json_path: "gaps/02/GAP2/gap.json".to_string(),
            },
            node_display_name: Some("Node B".to_string()),
            searchable_text: "Settings route Alice".to_string(),
            activity_ids: Vec::new(),
        },
    );
    server.projection.features.insert(
        "FEA1".to_string(),
        FeatureSummaryProjection {
            feature: FeatureIndexProjection {
                id: "FEA1".to_string(),
                name: "Settings Feature".to_string(),
                description: Some("Settings work".to_string()),
                reporter: Some("Alice".to_string()),
                node_id: Some("node-b".to_string()),
                created: "created".to_string(),
                updated: "updated".to_string(),
                json_path: "features/FE/A1/feature.json".to_string(),
            },
            status: GapStatus::Done,
            gap_ids: vec!["GAP2".to_string()],
            rollup: FeatureRollup {
                status: GapStatus::Done,
                gap_count: 1,
                done_count: 1,
                active_count: 0,
                failed_count: 0,
                cancelled_count: 0,
                blocked_count: 0,
                next_gap: None,
            },
        },
    );
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/gaps".to_string(),
        auth_token: None,
        body: None,
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["gaps"].as_array().unwrap().len(), 2);
    assert_eq!(response.body["counts"]["todo"], 1);
    assert_eq!(response.body["counts"]["done"], 1);

    let filtered = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps?reporter=Alice&feature=FEA1&rounds_gte=2&sort=priority&dir=desc&limit=1"
            .to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.body["gaps"][0]["id"], "GAP2");
    assert_eq!(filtered.body["filtered_counts"]["done"], 1);
    assert_eq!(filtered.body["matching_ids"], json!(["GAP2"]));
    assert_eq!(filtered.body["page"]["total"], 1);

    let features = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features?q=settings&status=done&reporter=Alice&node=node-b".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(features.status, 200);
    assert_eq!(features.body["features"][0]["feature"]["id"], "FEA1");
    assert_eq!(features.body["matching_ids"], json!(["FEA1"]));
}

#[test]
fn web_server_rejects_unauthorized_mutations() {
    let response = server_with_projection().handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        auth_token: None,
        body: None,
    });

    assert_eq!(response.status, 401);
    assert_eq!(response.body["error"]["code"], "unauthorized");
}

#[test]
fn web_server_issues_session_tokens_for_local_surface_mutations() {
    let temp_root = unique_temp_dir("http-session-auth");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.auth_token = None;
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let session = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/sessions".to_string(),
        auth_token: None,
        body: Some(json!({"surface": "desktop"})),
    });
    assert_eq!(session.status, 201);
    let token = session.body["session"]["token"]
        .as_str()
        .unwrap()
        .to_string();

    let create = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some(token),
        body: Some(json!({"id": "GAP1", "name": "Session API Gap"})),
    });
    assert_eq!(create.status, 201);
    assert!(durable_root.join("gaps/GA/P1/gap.json").exists());
    assert!(runtime_root.join("surface-sessions.json").exists());
    assert!(runtime_root.join("security-audit.jsonl").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_agent_secrets_with_local_auth() {
    let temp_root = unique_temp_dir("http-agent-secrets");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.auth_token = None;
    server.runtime_root = Some(runtime_root.clone());

    let unauthorized = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/secrets".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(unauthorized.status, 401);

    let session = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/sessions".to_string(),
        auth_token: None,
        body: Some(json!({"surface": "cli"})),
    });
    let token = session.body["session"]["token"]
        .as_str()
        .unwrap()
        .to_string();

    let put = server.handle(ApiRequest {
        method: "PUT".to_string(),
        path: "/api/agents/secrets/provider/smoke_ai_token".to_string(),
        auth_token: Some(token.clone()),
        body: Some(json!({"value": "secret-value"})),
    });
    assert_eq!(put.status, 200);
    assert_eq!(put.body["secret"]["name"], "smoke_ai_token");

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/secrets".to_string(),
        auth_token: Some(token.clone()),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["secrets"][0]["scope"], "provider");
    assert!(
        serde_json::to_string(&listed.body)
            .unwrap()
            .find("secret-value")
            .is_none()
    );

    let revealed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/secrets/provider/smoke_ai_token".to_string(),
        auth_token: Some(token.clone()),
        body: None,
    });
    assert_eq!(revealed.status, 200);
    assert_eq!(revealed.body["value"], "secret-value");

    let deleted = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/agents/secrets/provider/smoke_ai_token".to_string(),
        auth_token: Some(token),
        body: None,
    });
    assert_eq!(deleted.status, 200);
    assert!(runtime_root.join("secrets/secret-index.json").exists());

    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn local_http_daemon_validates_origin_version_and_idempotency_headers() {
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };

    let forbidden = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        headers: BTreeMap::from([("origin".to_string(), "https://example.com".to_string())]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(forbidden.status, 403);

    let version = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        headers: BTreeMap::from([("x-refine-api-version".to_string(), "999".to_string())]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(version.status, 426);

    let idempotency = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        headers: BTreeMap::from([("idempotency-key".to_string(), "bad key".to_string())]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(idempotency.status, 400);
}

#[test]
fn local_http_daemon_replays_idempotent_mutation_responses() {
    let temp_root = unique_temp_dir("http-idempotency");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let body = br#"{"id":"GAP1","name":"Idempotent Gap"}"#.to_vec();
    let headers = BTreeMap::from([
        ("authorization".to_string(), "Bearer secret".to_string()),
        ("idempotency-key".to_string(), "create-gap-1".to_string()),
    ]);

    let first = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        headers: headers.clone(),
        body: Some(body.clone()),
    });
    assert_eq!(first.status, 201);
    let second = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        headers: headers.clone(),
        body: Some(body),
    });
    assert_eq!(second.status, 201);
    assert_eq!(first.body, second.body);
    assert_eq!(
        fs::read_dir(durable_root.join("gaps/GA/P1"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "gap.json")
            .count(),
        1
    );
    assert!(
        runtime_root
            .join(IDEMPOTENCY_DIR)
            .join("create-gap-1.json")
            .exists()
    );
    let cached_projection: ProjectionSnapshot = serde_json::from_str(
        &fs::read_to_string(runtime_root.join("cache").join(PROJECTION_SNAPSHOT_FILE)).unwrap(),
    )
    .unwrap();
    assert!(cached_projection.gaps.contains_key("GAP1"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn local_http_daemon_rejects_idempotency_key_reuse_for_different_requests() {
    let temp_root = unique_temp_dir("http-idempotency-conflict");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root);
    server.runtime_root = Some(runtime_root);
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let headers = BTreeMap::from([
        ("authorization".to_string(), "Bearer secret".to_string()),
        (
            "idempotency-key".to_string(),
            "create-gap-conflict".to_string(),
        ),
    ]);

    let first = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        headers: headers.clone(),
        body: Some(br#"{"id":"GAP1","name":"First"}"#.to_vec()),
    });
    assert_eq!(first.status, 201);
    let conflict = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        headers,
        body: Some(br#"{"id":"GAP2","name":"Second"}"#.to_vec()),
    });
    assert_eq!(conflict.status, 409);
    let body: serde_json::Value = serde_json::from_slice(&conflict.body).unwrap();
    assert_eq!(body["error"]["code"], "idempotency_conflict");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn local_http_daemon_persists_successful_mutations_for_sse() {
    let temp_root = unique_temp_dir("http-mutation-sse");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root);
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };

    let create = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        headers: BTreeMap::from([("authorization".to_string(), "Bearer secret".to_string())]),
        body: Some(br#"{"id":"GAP1","name":"SSE Gap"}"#.to_vec()),
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
    assert!(body.contains("\"path\":\"/work/gaps\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn local_http_daemon_serves_projection_routes_over_tcp() {
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let handle = thread::spawn(move || daemon.serve_next(&listener).unwrap());

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(b"GET /work/gaps HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    handle.join().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("\"id\": \"GAP1\""));
    assert!(response.contains("\"counts\""));
}

#[test]
fn local_http_daemon_handles_tcp_requests_on_worker_threads() {
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let accept = thread::spawn(move || daemon.serve_next_concurrent(&listener).unwrap());

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(b"GET /system/version HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let worker = accept.join().unwrap();
    worker.join().unwrap().unwrap();

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("\"product\": \"refine\""));
}

#[test]
fn local_http_daemon_serves_static_assets() {
    let temp_root = unique_temp_dir("static-assets");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        temp_root.join("index.html"),
        "<!doctype html><title>Refine</title>",
    )
    .unwrap();
    fs::create_dir_all(temp_root.join("css")).unwrap();
    fs::write(temp_root.join("css/base.css"), "body { color: black; }").unwrap();
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: Some(temp_root.clone()),
    };

    let response = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "text/html; charset=utf-8");
    assert!(String::from_utf8(response.body).unwrap().contains("Refine"));

    let css = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/static/css/base.css".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(css.status, 200);
    assert_eq!(css.content_type, "text/css; charset=utf-8");
    assert!(
        String::from_utf8(css.body)
            .unwrap()
            .contains("color: black")
    );

    let traversal = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/static/../Cargo.toml".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(traversal.status, 400);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_transitions_gap_with_local_auth_and_durable_root() {
    let temp_root = unique_temp_dir("http-transition");
    let durable_root = temp_root.join(".refine");
    let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
    fs::create_dir_all(&gap_dir).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "HTTP transition",
              "status": "backlog",
              "priority": "low",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();
    let projection = FileProjectStateStore::new(&durable_root)
        .rebuild_projection()
        .unwrap();
    let mut server = server_with_projection();
    server.projection = projection;
    server.durable_root = Some(durable_root.clone());

    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/transition".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"status": "todo"})),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["gap"]["status"], "todo");
    assert!(
        fs::read_to_string(gap_dir.join("gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_creates_and_shows_gap_with_local_auth() {
    let temp_root = unique_temp_dir("http-create-show");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let create = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Created by API"})),
    });
    assert_eq!(create.status, 201);
    assert_eq!(create.body["gap"]["id"], "GAP1");
    assert!(durable_root.join("gaps/GA/P1/gap.json").exists());

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["gap"]["name"], "Created by API");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_edits_notes_and_deletes_gap_with_local_auth() {
    let temp_root = unique_temp_dir("http-edit-note-delete");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Original"})),
    });

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"name": "Renamed", "priority": "high"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["gap"]["name"], "Renamed");
    assert_eq!(edit.body["gap"]["priority"], "high");

    let note = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/notes".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"author": "Reviewer", "body": "Needs context"})),
    });
    assert_eq!(note.status, 200);
    let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"body\": \"Needs context\""));

    let delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(delete.status, 200);
    assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_appends_and_edits_latest_round_with_local_auth() {
    let temp_root = unique_temp_dir("http-rounds");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Round Gap"})),
    });

    let append = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/rounds".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"reporter": "Reporter", "actual": "Actual", "target": "Target"})),
    });
    assert_eq!(append.status, 200);
    assert_eq!(append.body["gap"]["round_count"], 1);

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/gaps/GAP1/rounds/latest".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"reporter": "Reviewer", "actual": "Revised"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["gap"]["reporter"], "Reviewer");
    let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"actual\": \"Revised\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_appends_and_reads_gap_round_logs_with_local_auth() {
    let temp_root = unique_temp_dir("http-gap-round-logs");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Logged Gap"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/rounds".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"reporter": "Reporter", "actual": "Actual", "target": "Target"})),
    });

    let append = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/rounds/0/logs".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "severity": "info",
            "category": "state",
            "actor": "refine",
            "message": "Workflow status changed: backlog -> todo"
        })),
    });
    assert_eq!(append.status, 200);
    assert!(durable_root.join("gaps/GA/P1/logs.jsonl").exists());

    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1/logs".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["round_log_count"], 1);
    assert_eq!(
        logs.body["logs"][0]["message"],
        "Workflow status changed: backlog -> todo"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_creates_features_and_updates_membership_with_local_auth() {
    let temp_root = unique_temp_dir("http-feature-membership");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Gap One"})),
    });

    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);
    assert_eq!(create_feature.body["feature"]["id"], "FEA1");

    let add_gap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"gap_id": "GAP1"})),
    });
    assert_eq!(add_gap.status, 200);
    assert_eq!(add_gap.body["gap_ids"], json!(["GAP1"]));

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["gap_ids"], json!(["GAP1"]));

    let remove_gap = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1/gaps/GAP1".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(remove_gap.status, 200);
    assert_eq!(remove_gap.body["gap_ids"], json!([]));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reorders_and_moves_feature_workflow_with_local_auth() {
    let temp_root = unique_temp_dir("http-feature-reorder-move");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for gap_id in ["GAP1", "GAP2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"gap_id": gap_id})),
        });
    }

    let reorder = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/gaps/GAP2/reorder".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"order": 1})),
    });
    assert_eq!(reorder.status, 200);
    assert_eq!(reorder.body["gap_ids"], json!(["GAP2", "GAP1"]));

    let move_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/move".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(move_feature.status, 200);
    assert_eq!(move_feature.body["rollup"]["status"], "todo");
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_updates_feature_metadata_and_runs_gap_actions() {
    let temp_root = unique_temp_dir("http-feature-gap-actions");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    for (id, name) in [
        ("GAP1", "Verify Gap"),
        ("GAP2", "Retry Quality"),
        ("GAP3", "Retry Merge"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "FEA1", "name": "Original Feature"})),
    });

    let feature = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/features/FEA1".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "name": "Renamed Feature",
            "description": "Updated description",
            "reporter": "QA"
        })),
    });
    assert_eq!(feature.status, 200);
    assert_eq!(feature.body["feature"]["name"], "Renamed Feature");
    assert_eq!(
        feature.body["feature"]["description"],
        "Updated description"
    );

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "selected_ids": ["GAP1"],
            "update": {"status": "review"}
        })),
    });
    let verified = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/verify".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(verified.status, 200);
    assert_eq!(verified.body["ok"], true);
    assert_eq!(verified.body["gap"]["status"], "done");

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "selected_ids": ["GAP2", "GAP3"],
            "update": {"status": "failed"}
        })),
    });
    let retry_quality = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP2/retry-quality".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(retry_quality.status, 200);
    assert_eq!(retry_quality.body["gap"]["status"], "qa");

    let retry_merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/retry-merge".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(retry_merge.status, 200);
    assert_eq!(retry_merge.body["gap"]["status"], "ready-merge");

    let merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/merge".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(merge.status, 200);
    assert_eq!(merge.body["gap"]["status"], "done");

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/undo".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(undo.status, 200);
    assert_eq!(undo.body["gap"]["status"], "review");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_schedules_workflow_through_file_scheduler_service() {
    let temp_root = unique_temp_dir("http-workflow-schedule");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Schedulable"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/transition".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"status": "todo"})),
    });

    let schedule = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/workflow/schedule".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(schedule.status, 200);
    assert_eq!(schedule.body["promoted"], 1);
    assert_eq!(schedule.body["reservations"][0]["gap_id"], "GAP1");
    assert!(runtime_root.join("scheduler-state.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_cancels_and_deletes_features_with_local_auth() {
    let temp_root = unique_temp_dir("http-feature-cancel-delete");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for gap_id in ["GAP1", "GAP2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"gap_id": gap_id})),
        });
    }

    let gap_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/cancel".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(gap_cancel.status, 200);
    assert_eq!(gap_cancel.body["gap"]["status"], "cancelled");

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .register(ManagedProcess {
            id: "agent-gap2".to_string(),
            owner: crate::core::host::process_supervision::ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("agent".to_string()),
            details: Some("working on GAP2".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "now".to_string(),
            exit_code: None,
        })
        .unwrap();
    let job = FileJobRegistry::new(&runtime_root)
        .register("feature FEA1 gap GAP2")
        .unwrap();

    let feature_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/cancel".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(feature_cancel.status, 200);
    assert_eq!(feature_cancel.body["rollup"]["cancelled_count"], 2);
    assert_eq!(feature_cancel.body["runtime_reconciled"]["processes"], 1);
    assert_eq!(feature_cancel.body["runtime_reconciled"]["jobs"], 1);
    assert_eq!(supervisor.inspect(&process.id).unwrap().state, "stopped");
    assert_eq!(
        FileJobRegistry::new(&runtime_root)
            .status(&job.id)
            .unwrap()
            .state,
        JobState::Cancelled
    );

    let feature_delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(feature_delete.status, 200);
    assert!(!durable_root.join("features/FE/A1/feature.json").exists());
    assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
    assert!(!durable_root.join("gaps/GA/P2/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_accepts_static_ui_api_aliases_for_work_routes() {
    let temp_root = unique_temp_dir("http-api-aliases");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let create_gap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Gap One"})),
    });
    assert_eq!(create_gap.status, 201);
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);

    let add_gap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/gaps/GAP1".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(add_gap.status, 200);
    assert_eq!(add_gap.body["gap_ids"], json!(["GAP1"]));

    let workflow = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/workflow".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(workflow.status, 200);
    assert_eq!(workflow.body["rollup"]["status"], "todo");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/cancel".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["gap"]["status"], "cancelled");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_accepts_static_ui_bulk_api_aliases() {
    let temp_root = unique_temp_dir("http-bulk-api-aliases");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    for (id, name) in [("GAP1", "Bulk One"), ("GAP2", "Bulk Two")] {
        let create = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": id, "name": name})),
        });
        assert_eq!(create.status, 201);
    }
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "FEA1", "name": "Bulk Feature"})),
    });
    assert_eq!(create_feature.status, 201);

    let bulk_status = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "selected_ids": ["GAP1", "GAP2"],
            "update": {"status": "todo"}
        })),
    });
    assert_eq!(bulk_status.status, 200);
    assert_eq!(bulk_status.body["updated"], 2);

    let bulk_assign = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/gaps/bulk".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"selected_ids": ["GAP1", "GAP2"]})),
    });
    assert_eq!(bulk_assign.status, 200);
    assert_eq!(bulk_assign.body["updated"], 2);
    assert_eq!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"feature_id\": \"FEA1\""),
        true
    );

    let bulk_delete = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk/delete".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"selected_ids": ["GAP1"]})),
    });
    assert_eq!(bulk_delete.status, 200);
    assert_eq!(bulk_delete.body["deleted"], 1);
    assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
    assert!(durable_root.join("gaps/GA/P2/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_records_and_lists_activity_for_static_ui() {
    let temp_root = unique_temp_dir("http-activity");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let recorded = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/ui-error".to_string(),
        auth_token: None,
        body: Some(json!({"message": "Boom", "source": "test"})),
    });
    assert_eq!(recorded.status, 200);
    assert_eq!(recorded.body["recorded"], true);
    assert!(durable_root.join("logs/activity.jsonl").exists());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["activity"][0]["message"], "Boom");
    assert_eq!(listed.body["facets"]["categories"], json!(["ui"]));

    let filtered = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity?q=source&limit=1".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.body["page"]["limit"], 1);
    assert_eq!(filtered.body["activity"][0]["message"], "Boom");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_parses_and_persists_imported_gaps_with_feature_destination() {
    let temp_root = unique_temp_dir("http-import-persist");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let parsed = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/import/csv/parse".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "text": "name,actual,target,reporter,priority\nCSV Gap,Actual state,Target state,QA,high\n"
            })),
        });
    assert_eq!(parsed.status, 200);
    assert_eq!(parsed.body["drafts"][0]["name"], "CSV Gap");
    assert_eq!(parsed.body["drafts"][0]["priority"], "high");

    let persisted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/persist".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "new_feature_name": "Imported Feature",
            "drafts": [{
                "name": "Imported Gap",
                "actual": "Actual state",
                "target": "Target state",
                "reporter": "QA",
                "priority": "high"
            }]
        })),
    });
    assert_eq!(persisted.status, 201);
    assert_eq!(persisted.body["count"], 1);
    assert_eq!(persisted.body["feature"]["name"], "Imported Feature");
    let gap_id = persisted.body["gaps"][0]["id"].as_str().unwrap();
    let feature_id = persisted.body["feature"]["id"].as_str().unwrap();

    let gap = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/gaps/{gap_id}"),
        auth_token: None,
        body: None,
    });
    assert_eq!(gap.status, 200);
    assert_eq!(gap.body["gap"]["priority"], "high");
    assert_eq!(gap.body["gap"]["reporter"], "QA");
    assert_eq!(gap.body["gap"]["round_count"], 1);
    assert_eq!(gap.body["gap"]["feature_id"], feature_id);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_rebuilds_projection_cache_and_serves_changes_performance_routes() {
    let temp_root = unique_temp_dir("http-cache-changes-performance");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Cached Gap"})),
    });
    let rebuilt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cache/rebuild".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"background": true})),
    });
    assert_eq!(rebuilt.status, 200);
    assert_eq!(rebuilt.body["gaps"], 1);
    assert!(
        runtime_root
            .join("cache")
            .join(PROJECTION_SNAPSHOT_FILE)
            .exists()
    );

    let changes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/changes?limit=10".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(changes.status, 200);
    assert_eq!(changes.body["branch"], "main");
    assert_eq!(changes.body["changes"], json!([]));

    let metrics = FileMetricsService::new(&runtime_root);
    metrics
        .record_operation(
            "cache.rebuild",
            25.0,
            true,
            json!({"resource_backend": "jsonl"}),
        )
        .unwrap();
    metrics
        .record_operation("provider.turn", 50.0, false, json!({}))
        .unwrap();
    let performance = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/performance?operation=cache.rebuild&limit=10".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(performance.status, 200);
    assert_eq!(performance.body["events"][0]["operation"], "cache.rebuild");
    assert_eq!(performance.body["filtered_event_count"], 1);
    assert_eq!(performance.body["total_event_count"], 2);
    assert_eq!(
        performance.body["operations"],
        json!(["cache.rebuild", "provider.turn"])
    );
    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached
            .runtime
            .performance
            .unwrap()
            .get("filtered_event_count"),
        Some(&json!(1))
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_lists_git_changes_and_reverts_commits() {
    let temp_root = unique_temp_dir("http-git-changes");
    let durable_root = temp_root.join(".refine");
    fs::create_dir_all(&durable_root).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "one\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(temp_root.join("app.txt"), "two\n").unwrap();
    git(&temp_root, &["commit", "-am", "update app"]).unwrap();

    let mut server = server_with_projection();
    server.durable_root = Some(durable_root);

    let changes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/changes?limit=5".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(changes.status, 200);
    assert_eq!(changes.body["changes"][0]["subject"], "update app");
    let commit = changes.body["changes"][0]["commit"].as_str().unwrap();

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/changes/undo".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"commit": commit})),
    });
    assert_eq!(undo.status, 200);
    assert_eq!(undo.body["ok"], true);
    assert_eq!(
        fs::read_to_string(temp_root.join("app.txt")).unwrap(),
        "one\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_hard_resets_git_worktree() {
    let temp_root = unique_temp_dir("http-git-reset");
    let durable_root = temp_root.join(".refine");
    fs::create_dir_all(&durable_root).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "committed\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(temp_root.join("app.txt"), "dirty\n").unwrap();

    let mut server = server_with_projection();
    server.durable_root = Some(durable_root);
    let reset = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(reset.status, 200);
    assert_eq!(reset.body["ok"], true);
    assert_eq!(
        fs::read_to_string(temp_root.join("app.txt")).unwrap(),
        "committed\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_cleans_activity_and_reports_unconnected_native_actions() {
    let temp_root = unique_temp_dir("http-cleanups");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/ui-error".to_string(),
        auth_token: None,
        body: Some(json!({"message": "Boom"})),
    });
    assert!(durable_root.join("logs/activity.jsonl").exists());
    let cleanup = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/cleanup".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"days": 0})),
    });
    assert_eq!(cleanup.status, 200);
    assert_eq!(cleanup.body["deleted"], 1);
    assert!(!durable_root.join("logs/activity.jsonl").exists());

    let runtime_root = temp_root.join("run/8080");
    server.runtime_root = Some(runtime_root.clone());
    let metrics = FileMetricsService::new(&runtime_root);
    metrics
        .record_operation("old", 10.0, true, json!({}))
        .unwrap();
    let performance_cleanup = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/performance/cleanup".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"clear": true})),
    });
    assert_eq!(performance_cleanup.status, 200);
    assert_eq!(performance_cleanup.body["deleted"], 1);
    assert!(!metrics.path().exists());

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/changes/undo".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"commit": "abc123"})),
    });
    assert_eq!(undo.status, 200);
    assert_eq!(undo.body["ok"], false);

    let reset = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(reset.status, 200);
    assert_eq!(reset.body["ok"], false);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_nodes_and_transfers_gap_ownership() {
    let temp_root = unique_temp_dir("http-node-transfer");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    for (id, name) in [("GAP1", "Transfer One"), ("GAP2", "Transfer Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": id, "name": name})),
        });
    }

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"display_name": "Remote QA"})),
    });
    assert_eq!(created.status, 200);
    assert!(
        created.body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "remote-qa")
    );

    let activated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/activate".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"node_id": "remote-qa"})),
    });
    assert_eq!(activated.status, 200);
    assert_eq!(activated.body["active_node_id"], "remote-qa");
    assert!(durable_root.join("active-node.json").exists());

    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "selected_ids": ["GAP1", "GAP2"],
            "target_node_id": "remote-qa"
        })),
    });
    assert_eq!(transfer.status, 200);
    assert_eq!(transfer.body["updated"], 2);
    let gap = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(gap.body["gap"]["node_id"], "remote-qa");

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/nodes/remote-qa".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"display_name": "Remote QA Renamed"})),
    });
    assert_eq!(renamed.status, 200);
    assert!(
        renamed.body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["display_name"] == "Remote QA Renamed")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_cluster_node_registry() {
    let temp_root = unique_temp_dir("http-cluster-registry");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cluster/nodes".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "id": "node-1",
            "display_name": "Node One",
            "ssh_host": "example.com",
            "ssh_user": "deploy",
            "ssh_identity_path": "~/.ssh/refine_ed25519",
            "target_app_path": "/srv/app"
        })),
    });
    assert_eq!(registered.status, 200);
    assert_eq!(registered.body["enabled"], true);
    assert_eq!(registered.body["nodes"][0]["ssh_host"], "example.com");
    assert_eq!(registered.body["nodes"][0]["ssh_user"], "deploy");
    assert_eq!(
        registered.body["nodes"][0]["ssh_identity_path"],
        "~/.ssh/refine_ed25519"
    );
    assert!(durable_root.join("cluster.json").exists());

    let disabled = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/cluster/nodes/node-1".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"enabled": false, "ssh_port": 2222})),
    });
    assert_eq!(disabled.status, 200);
    assert_eq!(disabled.body["nodes"][0]["enabled"], false);
    assert_eq!(disabled.body["nodes"][0]["ssh_port"], 2222);

    let bootstrap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cluster/nodes/node-1/bootstrap".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"dry_run": true})),
    });
    assert_eq!(bootstrap.status, 200);
    assert_eq!(bootstrap.body["ok"], true);
    assert_eq!(bootstrap.body["dry_run"], true);
    assert!(
        bootstrap.body["result"]["command"]
            .as_str()
            .unwrap()
            .contains("ssh -p 2222")
    );
    assert!(
        bootstrap.body["result"]["command"]
            .as_str()
            .unwrap()
            .contains("-i '~/.ssh/refine_ed25519'")
    );
    assert!(
        bootstrap.body["result"]["command"]
            .as_str()
            .unwrap()
            .contains("'deploy@example.com'")
    );
    assert_eq!(
        bootstrap.body["cluster"]["nodes"][0]["health"]["status"],
        "ready"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_serves_source_file_tree_read_and_search() {
    let temp_root = unique_temp_dir("http-files");
    let durable_root = temp_root.join(".refine");
    fs::create_dir_all(temp_root.join("src")).unwrap();
    fs::create_dir_all(&durable_root).unwrap();
    fs::write(temp_root.join("README.md"), "hello\nworld\n").unwrap();
    fs::write(temp_root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(durable_root.join("settings.json"), "{}").unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let tree = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/tree?path=&recursive=1&max_depth=2&max_entries=20".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(tree.status, 200);
    let root_entries = tree.body["entries_by_path"][""].as_array().unwrap();
    assert!(
        root_entries
            .iter()
            .any(|entry| entry["path"] == "README.md")
    );
    assert!(root_entries.iter().any(|entry| entry["path"] == "src"));
    assert!(!root_entries.iter().any(|entry| entry["path"] == ".refine"));
    assert!(
        tree.body["entries_by_path"]["src"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "src/main.rs")
    );

    let read = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=README.md&offset=0&limit=6".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(read.status, 200);
    assert_eq!(read.body["previewable"], true);
    assert_eq!(read.body["content"], "hello\n");
    assert_eq!(read.body["has_more"], true);
    assert_eq!(read.body["next_offset"], 6);

    let search = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/search?q=main&max_entries=5".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(search.status, 200);
    assert_eq!(search.body["entries"][0]["path"], "src/main.rs");

    let traversal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=../Cargo.toml".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(traversal.status, 400);
    assert_eq!(traversal.body["error"]["code"], "invalid_input");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_serves_project_utility_upgrade_health_and_sse_routes() {
    let temp_root = unique_temp_dir("http-project-utils");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(temp_root.join("child")).unwrap();
    fs::create_dir_all(&durable_root).unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let path = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!(
            "/api/project/path?path={}",
            percent_encode_for_test(temp_root.to_str().unwrap())
        ),
        auth_token: None,
        body: None,
    });
    assert_eq!(path.status, 200);
    assert_eq!(path.body["exists"], true);
    assert_eq!(path.body["is_dir"], true);

    let directories = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!(
            "/api/project/directories?path={}&max_entries=10",
            percent_encode_for_test(temp_root.to_str().unwrap())
        ),
        auth_token: None,
        body: None,
    });
    assert_eq!(directories.status, 200);
    assert!(
        directories.body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "child")
    );

    let upgrade = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/upgrade".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(upgrade.status, 200);
    assert_eq!(upgrade.body["upgrade"]["available"], false);
    assert_eq!(upgrade.body["upgrade"]["upgrade_available"], false);
    assert_eq!(upgrade.body["upgrade"]["local_development"], true);

    let install = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/install".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"target": "linux-cli-web", "version": "1.0.0"})),
    });
    assert_eq!(install.status, 200);
    assert_eq!(install.body["install"]["installed"], true);
    assert_eq!(install.body["install"]["target"], "linux_cli_web");

    let install_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/system/install".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(install_status.status, 200);
    assert_eq!(install_status.body["install"]["installed"], true);

    let update = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/update".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"version": "1.1.0"})),
    });
    assert_eq!(update.status, 200);
    assert_eq!(update.body["install"]["version"], "1.1.0");

    let rollback = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/rollback".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(rollback.status, 200);
    assert_eq!(rollback.body["install"]["version"], "1.0.0");

    let uninstall = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/uninstall".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(uninstall.status, 200);
    assert_eq!(uninstall.body["uninstalled"], true);

    let health = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/health".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(health.status, 200);
    assert_eq!(health.body["last_check_ok"], true);

    let job_registry = FileJobRegistry::new(&runtime_root);
    let job = job_registry.register("sse-job").unwrap();
    job_registry
        .append_log(
            &job.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "job".to_string(),
                message: "SSE job progress".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                gap_id: None,
            },
        )
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let stdout_path = runtime_root.join("sse.stdout.log");
    fs::write(&stdout_path, "SSE process output\n").unwrap();
    supervisor
        .register(ManagedProcess {
            id: "sse-process".to_string(),
            owner: crate::core::host::process_supervision::ProcessOwner::UserHelper,
            pid: None,
            state: "completed".to_string(),
            label: Some("sse".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    let chat = FileChatService::new(&durable_root);
    let session = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    chat.interrupt(&session.id, "SSE chat event").unwrap();

    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let sse = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/api/sse".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(sse.status, 200);
    assert_eq!(sse.content_type, "text/event-stream");
    let sse_body = String::from_utf8(sse.body).unwrap();
    assert!(sse_body.contains("event: ready"));
    assert!(sse_body.contains("event: project_updated"));
    assert!(sse_body.contains("event: status_change"));
    assert!(sse_body.contains("event: system_operation"));
    assert!(sse_body.contains("event: process_output"));
    assert!(sse_body.contains("SSE process output"));
    assert!(sse_body.contains("event: job_progress"));
    assert!(sse_body.contains("SSE job progress"));
    assert!(sse_body.contains("event: chat_event"));
    assert!(sse_body.contains("SSE chat event"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reads_and_cancels_runtime_jobs() {
    let temp_root = unique_temp_dir("http-jobs");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&durable_root).unwrap();
    let registry = FileJobRegistry::new(&runtime_root);
    let job = registry.register("bulk_update_gaps").unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/jobs/{}", job.id),
        auth_token: None,
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["job"]["status"], "running");
    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(cached.runtime.background_jobs[0]["id"], job.id);
    assert_eq!(cached.runtime.background_jobs[0]["status"], "running");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/jobs/{}/cancel", job.id),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["job"]["status"], "cancelled");
    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/jobs/{}/logs?limit=10", job.id),
        auth_token: None,
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["total"], 2);
    assert!(
        logs.body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["message"] == "Job cancelled")
    );
    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(cached.runtime.background_jobs[0]["status"], "cancelled");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_retries_scheduler_jobs_and_reads_retry_logs() {
    let temp_root = unique_temp_dir("http-job-retry");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&durable_root).unwrap();
    let scheduler = FileSchedulingService::new(&runtime_root);
    let reservation_id = scheduler.reserve("GAP1").unwrap();
    let job_id = scheduler.dispatch(&reservation_id).unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let retry = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/jobs/{job_id}/retry"),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body["retried_from"], job_id);
    assert_eq!(retry.body["job"]["owner"], "gap:GAP1");
    assert_ne!(retry.body["job"]["id"], job_id);

    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/jobs/{job_id}/logs?limit=1"),
        auth_token: None,
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["log_count"], 1);
    assert_eq!(logs.body["has_more"], true);
    assert_eq!(logs.body["total"], 2);

    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert!(
        cached
            .runtime
            .background_jobs
            .iter()
            .any(|job| job["id"] == retry.body["job"]["id"])
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_lists_processes_and_updates_pause_controls() {
    let temp_root = unique_temp_dir("http-processes");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&durable_root).unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .launch(crate::core::host::process_supervision::ManagedProcessSpec {
            owner: crate::core::host::process_supervision::ProcessOwner::Agent,
            command: if cfg!(windows) { "cmd" } else { "sh" }.to_string(),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "echo agent".to_string()]
            } else {
                vec!["-c".to_string(), "echo agent".to_string()]
            },
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
        })
        .unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["processes"][0]["kind"], "agent");
    assert_eq!(listed.body["runner_reachable"], true);
    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(cached.runtime.processes[0]["kind"], "agent");
    assert_eq!(
        cached.runtime.supervisor.unwrap()["runner_reachable"],
        json!(true)
    );

    let stdout_path = runtime_root.join("stream.stdout.log");
    let stderr_path = runtime_root.join("stream.stderr.log");
    fs::write(&stdout_path, "hello stdout\n").unwrap();
    fs::write(&stderr_path, "warn stderr\n").unwrap();
    supervisor
        .register(crate::core::host::process_supervision::ManagedProcess {
            id: "stream-test".to_string(),
            owner: crate::core::host::process_supervision::ProcessOwner::UserHelper,
            pid: None,
            state: "completed".to_string(),
            label: Some("stream".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    let stream = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes/stream-test/stream".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(stream.status, 200);
    assert_eq!(stream.body["process_id"], "stream-test");
    assert!(
        stream.body["output"]
            .as_str()
            .unwrap()
            .contains("hello stdout")
    );
    assert!(
        stream.body["output"]
            .as_str()
            .unwrap()
            .contains("warn stderr")
    );

    let background = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/background".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"stopped": true})),
    });
    assert_eq!(background.status, 200);
    assert_eq!(background.body["background_processes_stopped"], true);

    let agents = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/agents".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"paused": true})),
    });
    assert_eq!(agents.status, 200);
    assert_eq!(agents.body["agents_paused"], true);
    assert!(runtime_root.join("process-control.json").exists());
    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.runtime.supervisor.unwrap()["agents_paused"],
        json!(true)
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reports_provider_diagnostics_for_agents_and_recheck() {
    let server = server_with_projection();

    let agents = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(agents.status, 200);
    assert!(agents.body["providers"].as_array().unwrap().len() >= 5);
    assert_eq!(agents.body["stage"], "provider_detection");

    let diagnostics = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/smoke-ai/diagnostics".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(diagnostics.status, 200);
    assert_eq!(diagnostics.body["provider"], "smoke-ai");
    assert!(
        diagnostics.body["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry.as_str().unwrap_or("").contains("Smoke AI"))
    );

    let configured = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/agents/smoke-ai/configure".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(configured.status, 200);
    assert_eq!(configured.body["configured"], true);

    let auth = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/agents/smoke-ai/auth".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert!(auth.status == 200 || auth.status == 503);
    if auth.status == 503 {
        assert_eq!(auth.body["error"]["code"], "degraded");
    }

    let invalid = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/not-a-provider/diagnostics".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"]["code"], "invalid_input");

    let recheck = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/settings/recheck-auth".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(recheck.status, 200);
    assert!(recheck.body["message"].as_str().unwrap().contains("CLI"));
}

#[test]
fn web_server_manages_quality_settings_and_regressions() {
    let temp_root = unique_temp_dir("http-quality");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    write_fake_playwright(&temp_root, 0);
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let app_settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"target_app_url": "http://127.0.0.1:3000"})),
    });
    assert_eq!(app_settings.status, 200);

    let initial = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(initial.status, 200);
    assert_eq!(initial.body["enabled"], "0");
    assert_eq!(initial.body["timing"], "pre_merge");

    let saved = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/quality".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "enabled": "1",
            "timing": "post_rebuild",
            "regressions_enabled": true,
            "business_requirements": "Dashboard must render",
            "instructions": "Run focused checks"
        })),
    });
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["enabled"], "1");
    assert_eq!(saved.body["timing"], "post_rebuild");
    assert_eq!(saved.body["regressions_enabled"], "1");
    assert_eq!(saved.body["configured"], true);

    let checks = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/checks".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "owner_id": "GAP1",
            "command": "printf quality-ok"
        })),
    });
    assert_eq!(checks.status, 200);
    assert_eq!(checks.body["ok"], true);
    assert_eq!(checks.body["result"]["owner_id"], "GAP1");
    assert_eq!(checks.body["job"]["owner"], "quality:GAP1");
    assert_eq!(checks.body["job"]["status"], "complete");
    assert!(
        checks.body["result"]["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("quality-ok")
    );
    let quality_job_id = checks.body["job"]["id"].as_str().unwrap();
    let quality_job_logs = FileJobRegistry::new(&runtime_root)
        .page_logs(quality_job_id, 10, 0)
        .unwrap()
        .0;
    assert!(
        quality_job_logs
            .iter()
            .any(|log| log.message == "Quality checks passed")
    );

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/regressions".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "title": "Dashboard smoke",
            "prompt": "Open the dashboard",
            "description": "Dashboard scenario"
        })),
    });
    assert_eq!(created.status, 201);
    assert_eq!(created.body["regression"]["id"], "dashboard-smoke");
    assert!(
        durable_root
            .join("regressions/specs/dashboard-smoke.js")
            .exists()
    );

    let run = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/regressions/run".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({})),
    });
    assert_eq!(run.status, 200);
    assert_eq!(run.body["ok"], true);
    assert_eq!(run.body["runs"].as_array().unwrap().len(), 1);
    assert_eq!(
        run.body["runs"][0]["message"],
        "Playwright regression passed"
    );
    assert!(
        run.body["runs"][0]["json_report_path"]
            .as_str()
            .unwrap()
            .ends_with("report.json")
    );

    let screenshots = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality/screenshots?owner_id=GAP1".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(screenshots.status, 200);
    assert_eq!(screenshots.body["owner_id"], "GAP1");
    assert_eq!(screenshots.body["screenshot_count"], 1);
    assert!(
        screenshots.body["screenshots"][0]
            .as_str()
            .unwrap()
            .ends_with("screenshot.png")
    );

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["regressions"][0]["latest_run"]["ok"], true);

    let disabled = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/quality/regressions/dashboard-smoke".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"enabled": false})),
    });
    assert_eq!(disabled.status, 200);
    assert_eq!(disabled.body["regression"]["enabled"], false);

    let deleted = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/quality/regressions/dashboard-smoke".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(deleted.status, 200);
    assert_eq!(deleted.body["ok"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_durable_chat_sessions() {
    let temp_root = unique_temp_dir("http-chat");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    write_fake_provider(
        &durable_root,
        "smoke-ai",
        0,
        "{\"message\":\"web provider output\",\"importable_artifacts\":[{\"type\":\"round\",\"round\":{\"reporter\":\"QA\",\"actual\":\"Broken\",\"target\":\"Fixed\"}}]}",
    );
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/chat/start".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"gap_id": "GAP1", "provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201);
    let session_id = started.body["session_id"].as_str().unwrap().to_string();
    assert_eq!(started.body["mode"], "gap");
    assert!(
        durable_root
            .join(format!("chat/sessions/{session_id}.json"))
            .exists()
    );

    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/input"),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"text": "What should I test?"})),
    });
    assert_eq!(input.status, 200);

    let read = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/chat/{session_id}/read"),
        auth_token: None,
        body: None,
    });
    assert_eq!(read.status, 200);
    assert_eq!(read.body["alive"], true);
    assert!(
        read.body["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap_or("").contains("What should I test?"))
    );
    assert!(
        read.body["progress_lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line
                .as_str()
                .unwrap_or("")
                .contains("Provider turn completed"))
    );
    assert!(
        read.body["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap_or("").contains("web provider output"))
    );
    assert_eq!(
        read.body["importable_artifacts"].as_array().unwrap().len(),
        1
    );
    assert_eq!(read.body["importable_artifacts"][0]["type"], "round");
    assert_eq!(
        read.body["importable_artifacts"][0]["round"]["reporter"],
        "QA"
    );
    let jobs = FileJobRegistry::new(&runtime_root).recover().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].owner, format!("chat:{session_id}"));
    assert_eq!(jobs[0].state, JobState::Succeeded);
    let job = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/jobs/{}", jobs[0].id),
        auth_token: None,
        body: None,
    });
    assert_eq!(job.status, 200);
    assert_eq!(job.body["job"]["owner"], format!("chat:{session_id}"));
    assert_eq!(job.body["job"]["status"], "complete");

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/stop"),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(stopped.status, 200);
    assert_eq!(stopped.body["alive"], false);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn local_http_daemon_recovers_stale_chat_turns_before_serving() {
    let temp_root = unique_temp_dir("http-chat-recovery");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let chat = FileChatService::with_runtime_root(&durable_root, &runtime_root);
    let session = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    let job = FileJobRegistry::new(&runtime_root)
        .register(&format!("chat:{}", session.id))
        .unwrap();
    let session_path = durable_root.join(format!("chat/sessions/{}.json", session.id));

    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
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
        FileJobRegistry::new(&runtime_root)
            .status(&job.id)
            .unwrap()
            .state,
        JobState::Interrupted
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reports_project_registry_and_updates_settings() {
    let temp_root = unique_temp_dir("http-project-settings");
    let app_root = temp_root.join("app");
    let durable_root = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&durable_root).unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/project/status".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["attached"], true);
    assert_eq!(status.body["client_repo"], app_root.display().to_string());
    assert_eq!(status.body["apps"].as_array().unwrap().len(), 1);
    assert!(runtime_root.join("apps.json").exists());

    let app_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/apps/status".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(app_status.status, 200);
    assert_eq!(app_status.body["attached"], true);

    let other_app = temp_root.join("other");
    fs::create_dir_all(&other_app).unwrap();
    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"path": other_app.display().to_string()})),
    });
    assert_eq!(attached.status, 200);
    assert_eq!(
        attached.body["client_repo"],
        other_app.display().to_string()
    );

    let switched = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(switched.status, 200);
    assert_eq!(switched.body["client_repo"], app_root.display().to_string());

    let third_app = temp_root.join("third");
    fs::create_dir_all(&third_app).unwrap();
    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/register".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "name": "third-app",
            "path": third_app.display().to_string()
        })),
    });
    assert_eq!(registered.status, 201);
    assert_eq!(registered.body["apps"].as_array().unwrap().len(), 3);

    let clone_source = temp_root.join("clone-source");
    let clone_destination = temp_root.join("clone-destination");
    fs::create_dir_all(&clone_source).unwrap();
    let output = Command::new("git")
        .arg("init")
        .arg(&clone_source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cloned = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/clone".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "source": clone_source.display().to_string(),
            "destination": clone_destination.display().to_string(),
            "name": "cloned-app",
            "make_current": false
        })),
    });
    assert_eq!(cloned.status, 201);
    assert!(clone_destination.join(".git").exists());
    assert_eq!(cloned.body["apps"].as_array().unwrap().len(), 4);

    let switched_by_name = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"name": "third-app"})),
    });
    assert_eq!(switched_by_name.status, 200);
    assert_eq!(
        switched_by_name.body["client_repo"],
        third_app.display().to_string()
    );

    let detached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/detach".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(detached.status, 200);
    assert_eq!(detached.body["attached"], false);
    assert_eq!(detached.body["client_repo"], serde_json::Value::Null);

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/apps".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["apps"].as_array().unwrap().len(), 4);
    assert_eq!(listed.body["current"], "");

    let settings = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/settings".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "claude");
    assert_eq!(settings.body["runtime"]["paused"], false);

    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "agent_cli": "smoke-ai",
            "parallel_run_cap": 3,
            "paused": true
        })),
    });
    assert_eq!(updated.status, 200);
    assert_eq!(updated.body["settings"]["agent_cli"], "smoke-ai");
    assert_eq!(updated.body["settings"]["parallel_run_cap"], "3");
    assert_eq!(updated.body["settings"]["paused"], "1");
    assert_eq!(updated.body["runtime"]["agents_paused"], true);
    assert_eq!(
        updated.body["runtime"]["background_processes_stopped"],
        true
    );
    assert!(runtime_root.join("process-control.json").exists());
    assert!(durable_root.join("settings.json").exists());

    let removed = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/apps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"path": other_app.display().to_string()})),
    });
    assert_eq!(removed.status, 200);
    assert_eq!(removed.body["apps"].as_array().unwrap().len(), 3);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_resolves_app_scoped_routes_from_active_runtime_app() {
    let temp_root = unique_temp_dir("http-detached-active-app");
    let app_root = temp_root.join("app");
    let durable_root = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&durable_root).unwrap();
    let mut server = server_with_projection();
    server.durable_root = None;
    server.runtime_root = Some(runtime_root.clone());

    let detached_settings = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/settings".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(detached_settings.status, 503);
    assert_eq!(
        detached_settings.body["error"]["code"],
        "durable_root_unavailable"
    );

    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(attached.status, 200);
    assert_eq!(attached.body["client_repo"], app_root.display().to_string());

    let settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"agent_cli": "smoke-ai"})),
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "smoke-ai");
    assert!(durable_root.join("settings.json").exists());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"name": "Detached attach gap", "id": "GAP1"})),
    });
    assert_eq!(created.status, 201);
    assert!(durable_root.join("gaps/GA/P1/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_governance_guidance_and_reporters() {
    let temp_root = unique_temp_dir("http-project-config");
    let durable_root = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());

    let governance = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/governance".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "product": "Refine",
            "constitution": "Be useful",
            "rules": [{"text": "No regressions"}]
        })),
    });
    assert_eq!(governance.status, 200);
    assert_eq!(governance.body["configured"], true);
    assert_eq!(governance.body["rules"].as_array().unwrap().len(), 1);

    let generated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/governance/generate-rules".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"product": "Refine", "constitution": "Be useful"})),
    });
    assert_eq!(generated.status, 200);
    assert_eq!(generated.body["ok"], true);
    assert!(generated.body["rules"].as_array().unwrap().len() >= 2);

    let guidance = server.handle(ApiRequest {
        method: "PUT".to_string(),
        path: "/api/guidance".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"guidance": [{
            "name": "Accessibility",
            "rule": "When UI changes",
            "instructions": "Check keyboard behavior",
            "enabled": true
        }]})),
    });
    assert_eq!(guidance.status, 200);
    assert_eq!(guidance.body["guidance"].as_array().unwrap().len(), 1);

    let reporter_one = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/reporters".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"name": "Buddy"})),
    });
    assert_eq!(reporter_one.status, 201);
    let reporter_one_id = reporter_one.body["reporter"]["id"].as_u64().unwrap();
    let reporter_two = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/reporters".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"name": "Alex"})),
    });
    let reporter_two_id = reporter_two.body["reporter"]["id"].as_u64().unwrap();

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/reporters/{reporter_one_id}"),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"name": "Buddy Williams"})),
    });
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.body["new"], "Buddy Williams");

    let merged = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/reporters/{reporter_one_id}/merge"),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"target_id": reporter_two_id})),
    });
    assert_eq!(merged.status, 200);
    assert_eq!(merged.body["ok"], true);

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/reporters".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["reporters"].as_array().unwrap().len(), 1);
    assert!(durable_root.join("governance.json").exists());
    assert!(durable_root.join("guidance.json").exists());
    assert!(durable_root.join("reporters.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reports_dashboard_diagnostics_target_app_nodes_and_cluster() {
    let temp_root = unique_temp_dir("http-status-surfaces");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        temp_root.join("package.json"),
        r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
    )
    .unwrap();
    let mut server = server_with_projection();
    server.durable_root = Some(durable_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "target_app_start_command": "npm run dev",
            "target_app_auto_rebuild": "never"
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"id": "GAP1", "name": "Dashboard Gap"})),
    });
    FileActivityService::new(&durable_root)
        .append(ActivityEntry {
            id: "act-dashboard".to_string(),
            datetime: "2026-06-05T00:00:00Z".to_string(),
            severity: "info".to_string(),
            category: "state".to_string(),
            message: "Dashboard activity".to_string(),
            gap_id: Some("GAP1".to_string()),
            actor: Some("system".to_string()),
            details: None,
            actions: Vec::new(),
        })
        .unwrap();

    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=current".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body["counts"]["backlog"], 1);
    assert_eq!(dashboard.body["active_node_id"], "default");
    assert_eq!(dashboard.body["activity"][0]["id"], "act-dashboard");
    let cached = FileProjectStateStore::new(&durable_root)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.runtime.target_app.unwrap()["app_url"],
        "http://127.0.0.1:3000"
    );
    assert_eq!(
        cached.runtime.preflight.unwrap()["stage"],
        "provider_detection"
    );

    let diagnostics = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/diagnostics".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(diagnostics.status, 200);
    assert_eq!(diagnostics.body["reachable"], true);
    for key in [
        "daemon",
        "install",
        "os_backend",
        "target_app",
        "git",
        "provider",
        "browser",
        "docker",
        "storage",
    ] {
        assert!(
            diagnostics.body["doctor"][key]
                .as_array()
                .map(|items| !items.is_empty())
                .unwrap_or(false),
            "missing doctor section {key}"
        );
    }
    assert!(
        diagnostics.body["doctor"]["target_app"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry
                .as_str()
                .unwrap_or("")
                .contains("supervised by the native daemon"))
    );
    assert!(
        diagnostics.body["doctor"]["storage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry
                .as_str()
                .unwrap_or("")
                .contains("runtime_root_exists="))
    );

    let target = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/target-app/status".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(target.status, 200);
    assert_eq!(target.body["app_url"], "http://127.0.0.1:3000");
    assert_eq!(target.body["has_start_command"], true);

    let generated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/generate-instructions".to_string(),
        auth_token: Some("secret".to_string()),
        body: Some(json!({"kind": "all"})),
    });
    assert_eq!(generated.status, 200);
    assert_eq!(generated.body["config"]["start_command"], "npm run dev");
    assert_eq!(
        generated.body["settings"]["target_app_rebuild_command"],
        "npm run build"
    );
    assert_eq!(generated.body["config"]["tcp_check_port"], "3000");

    let rebuild = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/target-app-rebuilder/rebuild".to_string(),
        auth_token: Some("secret".to_string()),
        body: None,
    });
    assert_eq!(rebuild.status, 200);
    assert_eq!(rebuild.body["queued"], false);

    let nodes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/nodes".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(nodes.status, 200);
    assert_eq!(nodes.body["nodes"][0]["id"], "default");

    let cluster = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/cluster".to_string(),
        auth_token: None,
        body: None,
    });
    assert_eq!(cluster.status, 200);
    assert_eq!(cluster.body["enabled"], false);

    fs::remove_dir_all(temp_root).unwrap();
}

fn server_with_projection() -> InProcessWebServer {
    let mut gaps = BTreeMap::new();
    gaps.insert(
        "GAP1".to_string(),
        GapSummaryProjection {
            gap: GapIndexProjection {
                id: "GAP1".to_string(),
                name: "Projection route".to_string(),
                status: GapStatus::Todo,
                priority: GapPriority::Medium,
                reporter: Some("Buddy".to_string()),
                round_count: 1,
                created: "created".to_string(),
                updated: "updated".to_string(),
                branch_name: None,
                node_id: Some("default".to_string()),
                feature_id: None,
                feature_order: None,
                json_path: "gaps/01/GAP1/gap.json".to_string(),
            },
            node_display_name: None,
            searchable_text: "Projection route".to_string(),
            activity_ids: Vec::new(),
        },
    );

    InProcessWebServer {
        status: DaemonStatus {
            port: 8080,
            daemon_healthy: true,
            web_available: true,
            worker_state: "idle".to_string(),
            target_app_state: "detached".to_string(),
            active_operations: Vec::new(),
            degraded_integrations: Vec::new(),
        },
        projection: ProjectionSnapshot {
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "now".to_string(),
            source_fingerprints: BTreeMap::new(),
            gaps,
            features: BTreeMap::new(),
            activity: BTreeMap::new(),
            changes: BTreeMap::new(),
            dashboard: DashboardProjection::default(),
            runtime: RuntimeProjection::default(),
        },
        auth_token: Some("secret".to_string()),
        durable_root: None,
        runtime_root: None,
    }
}

fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RefineError::Conflict(
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .trim()
            .to_string(),
        ))
    }
}

fn write_fake_playwright(root: &Path, exit_code: i32) {
    let bin_dir = root.join("node_modules/.bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join("playwright");
    let mut file = fs::File::create(&path).unwrap();
    writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' '{{\"status\":\"passed\"}}'\nif [ -n \"$REFINE_REGRESSION_SCREENSHOT\" ]; then printf 'png' > \"$REFINE_REGRESSION_SCREENSHOT\"; fi\nexit {exit_code}"
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
}

fn write_fake_provider(durable_root: &Path, name: &str, exit_code: i32, output: &str) {
    let bin_dir = durable_root.join("provider-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    writeln!(
        file,
        "#!/bin/sh\nprintf '%s\\n' {output:?}\nexit {exit_code}"
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-native-{prefix}-{}-{nanos}",
        std::process::id()
    ))
}

fn percent_encode_for_test(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
