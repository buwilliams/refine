use crate::model::log::LogEntry;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::tools::product::chat::{ChatAttachment, ChatService, FileChatService};
use crate::workflow::{WorkflowAutomation, WorkflowEngine};
use serde_json::json;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::*;
use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::gap::{GapIndexProjection, GapPriority};
use crate::model::log::ActivityEntry;
use crate::model::workflow::GapStatus;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner, ProcessSupervisor,
};
use crate::surfaces::web_server::support::{
    runtime_process_status_value, runtime_process_summary_value,
};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::product::project_state::{
    DashboardProjection, FeatureSummaryProjection, FileProjectStateStore, GapSummaryProjection,
    PROJECTION_SNAPSHOT_FILE, PROJECTION_SNAPSHOT_VERSION, ProjectStateStore, ProjectionSnapshot,
    RuntimeProjection,
};
use crate::tools::product::work_items::FileWorkItemService;

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
                assignee: Some("Alice".to_string()),
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
                assignee: Some("Alice".to_string()),
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
        body: None,
    });
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.body["gaps"][0]["id"], "GAP2");
    assert_eq!(filtered.body["filtered_counts"]["done"], 1);
    assert_eq!(filtered.body["matching_ids"], json!(["GAP2"]));
    assert_eq!(filtered.body["page"]["total"], 1);
    assert!(filtered.body.get("facets").is_none());

    let status_facets = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps?status=todo&reporter=Alice&feature=FEA1&rounds_gte=2&facets=1".to_string(),
        body: None,
    });
    assert_eq!(status_facets.status, 200);
    assert_eq!(status_facets.body["gaps"].as_array().unwrap().len(), 0);
    assert_eq!(
        status_facets.body["filtered_counts"]
            .as_object()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(status_facets.body["facets"]["status_counts"]["done"], 1);

    let features = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features?q=settings&status=done&reporter=Alice&node=node-b".to_string(),
        body: None,
    });
    assert_eq!(features.status, 200);
    assert_eq!(features.body["features"][0]["feature"]["id"], "FEA1");
    assert_eq!(features.body["matching_ids"], json!(["FEA1"]));
}

#[test]
fn web_server_structures_dashboard_attention_and_runtime_banner() {
    let mut server = server_with_projection();
    server
        .projection
        .dashboard
        .attention_indicators
        .push("1 failed Gap(s) need recovery".to_string());
    server.projection.runtime.supervisor = json!({"runner_reachable": false}).as_object().cloned();

    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(response.status, 200);
    assert_eq!(response.body["runner_reachable"], json!(false));
    let attention = response.body["needs_attention"].as_array().unwrap();
    assert!(attention.iter().any(|item| {
        item["kind"] == "filter"
            && item["message"] == "1 failed Gap(s) need recovery"
            && item["severity"] == "warn"
    }));
    assert!(attention.iter().any(|item| {
        item["kind"] == "banner"
            && item["severity"] == "error"
            && item["message"]
                .as_str()
                .unwrap()
                .contains("Refine cannot reach the runtime worker")
    }));
}

#[test]
fn runtime_process_status_counts_only_current_agents() {
    let mut runtime = RuntimeProjection {
        supervisor: json!({"runner_reachable": true}).as_object().cloned(),
        ..RuntimeProjection::default()
    };
    runtime.processes = vec![
        json!({
            "id": "exited-agent",
            "kind": "agent",
            "status": "exited"
        })
        .as_object()
        .cloned()
        .unwrap(),
        json!({
            "id": "completed-agent",
            "kind": "agent",
            "status": "completed"
        })
        .as_object()
        .cloned()
        .unwrap(),
        json!({
            "id": "running-chat",
            "kind": "chat",
            "status": "running"
        })
        .as_object()
        .cloned()
        .unwrap(),
        json!({
            "id": "stopped-ui",
            "kind": "ui",
            "status": "stopped"
        })
        .as_object()
        .cloned()
        .unwrap(),
    ];

    let status = runtime_process_status_value(&runtime);
    assert_eq!(status["agent_count"], 0);
    assert_eq!(status["process_count"], 1);
    assert_eq!(status["running_process_count"], 1);

    let summary = runtime_process_summary_value(&runtime);
    let processes = summary["processes"].as_array().unwrap();
    assert_eq!(processes.len(), 1);
    assert!(
        processes
            .iter()
            .any(|process| process["id"] == "running-chat")
    );
    assert!(
        processes
            .iter()
            .all(|process| process["id"] != "stopped-ui")
    );
    assert!(
        !processes
            .iter()
            .any(|process| process["id"] == "exited-agent")
    );
    assert!(
        !processes
            .iter()
            .any(|process| process["id"] == "completed-agent")
    );
}

#[test]
fn web_server_route_groups_cover_static_web_surface() {
    let groups = API_GROUPS
        .iter()
        .map(|group| group.prefix)
        .collect::<std::collections::BTreeSet<_>>();
    for prefix in [
        "/activity",
        "/agents",
        "/apps",
        "/cache",
        "/changes",
        "/chat",
        "/cluster",
        "/dashboard",
        "/diagnostics",
        "/events",
        "/files",
        "/governance",
        "/guidance",
        "/import",
        "/operations",
        "/nodes",
        "/performance",
        "/processes",
        "/project",
        "/quality",
        "/reporters",
        "/runner-workers",
        "/settings",
        "/system",
        "/target-app",
        "/upgrade",
        "/work",
        "/workflow",
    ] {
        assert!(groups.contains(prefix), "missing route group {prefix}");
    }

    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static/js");
    let guide = fs::read_to_string(static_root.join("features/guide.js")).unwrap();
    let guide_ids = extract_prefixed_string_literals(&guide, "guideItem(\"")
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut settings_ids = std::collections::BTreeSet::new();
    for entry in fs::read_dir(static_root.join("features")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("settings") || !name.ends_with(".js") {
            continue;
        }
        let source = fs::read_to_string(entry.path()).unwrap();
        settings_ids.extend(extract_settings_guide_label_ids(&source));
        settings_ids.extend(extract_prefixed_string_literals(&source, "guideItemId: \""));
    }
    let missing_ids = settings_ids
        .difference(&guide_ids)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing_ids.is_empty(),
        "settings Guide labels without guideItem targets: {missing_ids:?}"
    );

    let guide_hashes = extract_prefixed_string_literals(&guide, "hash: \"");
    let stale_hashes = guide_hashes
        .into_iter()
        .filter(|hash| {
            hash.starts_with("#/system")
                || hash.starts_with("#/settings")
                || hash.starts_with("#/project/application")
                || hash.starts_with("#/node/nodes")
                || hash.contains("application-config")
                || hash.contains("target-app-config")
        })
        .collect::<Vec<_>>();
    assert!(
        stale_hashes.is_empty(),
        "Guide targets point at removed screen locations: {stale_hashes:?}"
    );
}

fn extract_prefixed_string_literals(source: &str, prefix: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = source;
    while let Some(idx) = rest.find(prefix) {
        let after = &rest[idx + prefix.len()..];
        let Some(end) = after.find('"') else { break };
        values.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    values
}

fn extract_settings_guide_label_ids(source: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = source;
    while let Some(idx) = rest.find("renderSettingsGuideLabel(") {
        let after = &rest[idx + "renderSettingsGuideLabel(".len()..];
        if !after.trim_start().starts_with('"') {
            rest = &after[1..];
            continue;
        }
        let window = &after[..after.len().min(600)];
        let literals = string_literals(window);
        if let Some(id) = literals.get(1).filter(|id| !id.is_empty()) {
            ids.push(id.clone());
        }
        rest = &after[1..];
    }
    ids
}

fn string_literals(source: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut value = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            if escaped {
                value.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                break;
            }
            value.push(ch);
        }
        values.push(value);
    }
    values
}

#[test]
fn web_server_manages_agent_secrets() {
    let temp_root = unique_temp_dir("http-agent-secrets");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());

    let put = server.handle(ApiRequest {
        method: "PUT".to_string(),
        path: "/api/agents/secrets/provider/smoke_ai_token".to_string(),
        body: Some(json!({"value": "secret-value"})),
    });
    assert_eq!(put.status, 200);
    assert_eq!(put.body["secret"]["name"], "smoke_ai_token");

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/secrets".to_string(),
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
        body: None,
    });
    assert_eq!(revealed.status, 200);
    assert_eq!(revealed.body["value"], "secret-value");

    let deleted = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/agents/secrets/provider/smoke_ai_token".to_string(),
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
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let body = br#"{"id":"GAP1","name":"Idempotent Gap"}"#.to_vec();
    let headers = BTreeMap::from([("idempotency-key".to_string(), "create-gap-1".to_string())]);

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
        fs::read_dir(refine_dir.join("gaps/GA/P1"))
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
fn web_server_creates_gap_from_new_gap_modal_payload() {
    let temp_root = unique_temp_dir("http-gap-create-modal");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "actual": "The game does not pause when the pause key is pressed.",
            "target": "Pressing pause should freeze the board and show a paused state.",
            "priority": "high"
        })),
    });

    assert_eq!(created.status, 201);
    let gap_id = created.body["gap"]["id"].as_str().unwrap();
    assert_eq!(
        created.body["gap"]["name"],
        "Pressing pause should freeze the board and show a paused state."
    );
    assert_eq!(created.body["gap"]["priority"], "high");
    assert_eq!(created.body["gap"]["reporter"], "Alice");
    assert_eq!(created.body["gap"]["round_count"], 1);

    let detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/gaps/{gap_id}"),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(
        detail.body["gap"]["rounds"][0]["actual"],
        "The game does not pause when the pause key is pressed."
    );
    assert_eq!(
        detail.body["gap"]["rounds"][0]["target"],
        "Pressing pause should freeze the board and show a paused state."
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_handles_new_gap_duplicate_decisions() {
    let temp_root = unique_temp_dir("http-gap-duplicate-modal");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let body = json!({
        "reporter": "Alice",
        "actual": "Duplicate actual state",
        "target": "Duplicate target state",
        "priority": "low"
    });
    let original = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(body.clone()),
    });
    assert_eq!(original.status, 201);
    let original_id = original.body["gap"]["id"].as_str().unwrap().to_string();

    let duplicate = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(body.clone()),
    });
    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.body["error"]["code"], "duplicate_gap");
    assert_eq!(
        duplicate.body["error"]["duplicate"]["match"]["id"],
        original_id
    );

    let ignored = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "actual": "Duplicate actual state",
            "target": "Duplicate target state",
            "duplicate_decision": "duplicate"
        })),
    });
    assert_eq!(ignored.status, 200);
    assert_eq!(ignored.body["created"], false);
    assert_eq!(ignored.body["duplicate_action"], "duplicate");

    let imported = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "actual": "Duplicate actual state",
            "target": "Duplicate target state",
            "duplicate_decision": "original"
        })),
    });
    assert_eq!(imported.status, 201);
    let imported_id = imported.body["gap"]["id"].as_str().unwrap();
    assert_ne!(imported_id, original_id);

    let list = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps?q=Duplicate%20actual%20state".to_string(),
        body: None,
    });
    assert_eq!(list.status, 200);
    assert_eq!(list.body["page"]["total"], 2);

    fs::remove_dir_all(temp_root).unwrap();
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
        "create-gap-conflict".to_string(),
    )]);

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
        path: "/api/gaps".to_string(),
        headers: BTreeMap::new(),
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
    let handle = thread::spawn(move || daemon.serve_once(listener).unwrap());

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
fn local_http_daemon_handles_tcp_requests_on_worker_threads() {
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };
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
    daemon.recover_runtime_state().unwrap();

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

    thread::sleep(Duration::from_millis(10));
    fs::write(temp_root.join("css/base.css"), "body { color: blue; }").unwrap();
    let updated_css = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/static/css/base.css".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(updated_css.status, 200);
    assert!(
        String::from_utf8(updated_css.body)
            .unwrap()
            .contains("color: blue")
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
fn local_http_daemon_reports_startup_cache_progress() {
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: None,
    };
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
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    daemon.recover_runtime_state().unwrap();

    let create = daemon.handle_wire_request(HttpRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        headers: BTreeMap::new(),
        body: Some(br#"{"id":"HOT1","name":"Hot cached Gap"}"#.to_vec()),
    });
    assert_eq!(create.status, 201);

    let list = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/api/gaps?limit=50&offset=0".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(list.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&list.body).unwrap();
    assert_eq!(body["gaps"][0]["id"], "HOT1");

    let events = wait_for_http_request_metrics(&runtime_root);
    assert!(events.iter().any(|event| {
        event.operation == "http.request"
            && event.details.get("path").and_then(|value| value.as_str()) == Some("/work/gaps")
    }));

    for path in [
        "/api/dashboard?node=current",
        "/api/gaps?limit=50&offset=0",
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
        assert!(
            elapsed < Duration::from_millis(75),
            "{path} took {:?}",
            elapsed
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_transitions_gap_and_refine_dir() {
    let temp_root = unique_temp_dir("http-transition");
    let refine_dir = temp_root.join(".refine");
    let gap_dir = refine_dir.join("gaps").join("01").join("GAP1");
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
    let projection = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let mut server = server_with_projection();
    server.projection = projection;
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/transition".to_string(),
        body: Some(json!({"status": "todo"})),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["gap"]["status"], "todo");
    assert!(
        fs::read_to_string(gap_dir.join("gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    let patch_response = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/gaps/GAP1".to_string(),
        body: Some(json!({"status": "backlog"})),
    });
    assert_eq!(patch_response.status, 200);
    assert_eq!(patch_response.body["gap"]["status"], "backlog");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_creates_and_shows_gap() {
    let temp_root = unique_temp_dir("http-create-show");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let create = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Created by API"})),
    });
    assert_eq!(create.status, 201);
    assert_eq!(create.body["gap"]["id"], "GAP1");
    assert!(refine_dir.join("gaps/GA/P1/gap.json").exists());

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["gap"]["name"], "Created by API");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_edits_notes_and_deletes_gap() {
    let temp_root = unique_temp_dir("http-edit-note-delete");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Original"})),
    });

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: Some(json!({"name": "Renamed", "priority": "high"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["gap"]["name"], "Renamed");
    assert_eq!(edit.body["gap"]["priority"], "high");

    let note = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/notes".to_string(),
        body: Some(json!({"author": "Reviewer", "body": "Needs context"})),
    });
    assert_eq!(note.status, 200);
    let written = fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"body\": \"Needs context\""));
    let written_gap = serde_json::from_str::<serde_json::Value>(&written).unwrap();
    let note_id = written_gap["notes"][0]["id"].as_str().unwrap();

    let edited_note = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: Some(json!({
            "notes": [{
                "id": note_id,
                "author": "Reviewer",
                "body": "Updated context",
                "created": written_gap["notes"][0]["created"].clone()
            }]
        })),
    });
    assert_eq!(edited_note.status, 200);
    let edited_detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(
        edited_detail.body["gap"]["notes"][0]["body"],
        "Updated context"
    );

    let deleted_note = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: Some(json!({"notes": []})),
    });
    assert_eq!(deleted_note.status, 200);
    let deleted_detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(deleted_detail.body["gap"]["notes"], json!([]));

    let delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(delete.status, 200);
    assert!(!refine_dir.join("gaps/GA/P1/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_appends_and_edits_latest_round() {
    let temp_root = unique_temp_dir("http-rounds");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Round Gap"})),
    });

    let append = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/rounds".to_string(),
        body: Some(json!({"reporter": "Reporter", "actual": "Actual", "target": "Target"})),
    });
    assert_eq!(append.status, 200);
    assert_eq!(append.body["gap"]["round_count"], 1);

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/gaps/GAP1/rounds/latest".to_string(),
        body: Some(json!({"reporter": "Reviewer", "assignee": "Reviewer", "actual": "Revised"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["gap"]["reporter"], "Reviewer");
    let written = fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"actual\": \"Revised\""));

    let detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(detail.body["gap"]["round_count"], 1);
    assert_eq!(detail.body["gap"]["rounds"][0]["reporter"], "Reviewer");
    assert_eq!(detail.body["gap"]["rounds"][0]["assignee"], "Reviewer");
    assert_eq!(detail.body["gap"]["rounds"][0]["actual"], "Revised");

    let reporters = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/reporters".to_string(),
        body: None,
    });
    assert_eq!(reporters.status, 200);
    assert_eq!(reporters.body["reporters"][0]["name"], "Reviewer");
    assert!(refine_dir.join("reporters.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_appends_and_reads_gap_round_logs() {
    let temp_root = unique_temp_dir("http-gap-round-logs");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Logged Gap"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/rounds".to_string(),
        body: Some(json!({"reporter": "Reporter", "actual": "Actual", "target": "Target"})),
    });

    let append = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/rounds/0/logs".to_string(),
        body: Some(json!({
            "severity": "info",
            "category": "state",
            "actor": "refine",
            "message": "Workflow status changed: backlog -> todo"
        })),
    });
    assert_eq!(append.status, 200);
    assert!(refine_dir.join("gaps/GA/P1/logs.jsonl").exists());

    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1/logs".to_string(),
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["round_log_count"], 1);
    assert_eq!(
        logs.body["logs"][0]["message"],
        "Workflow status changed: backlog -> todo"
    );

    let evaluation = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/gaps/GAP1/rounds/latest/evaluation".to_string(),
        body: Some(json!({
            "rule_state": "failed",
            "product_state": "fail",
            "constitution_state": "pass",
            "meta_rule_state": "needs-review",
            "governance_message": "Governance found a product concern.",
            "governance_details": "Product requirement mismatch",
            "governance_rule_actions": [{"action": "flag", "text": "Update policy"}],
            "quality_state": "failed",
            "quality_message": "Quality check failed.",
            "quality_details": "Screenshot mismatch",
            "quality_checked_at": "2026-06-07T22:00:00Z"
        })),
    });
    assert_eq!(evaluation.status, 200);
    let detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(detail.body["gap"]["rounds"][0]["rule_state"], "failed");
    assert_eq!(
        detail.body["gap"]["rounds"][0]["governance_message"],
        "Governance found a product concern."
    );
    assert_eq!(detail.body["gap"]["rounds"][0]["quality_state"], "failed");
    assert_eq!(
        detail.body["gap"]["rounds"][0]["quality_message"],
        "Quality check failed."
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_creates_features_and_updates_membership() {
    let temp_root = unique_temp_dir("http-feature-membership");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Gap One"})),
    });

    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);
    assert_eq!(create_feature.body["feature"]["id"], "FEA1");

    let add_gap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/gaps".to_string(),
        body: Some(json!({"gap_id": "GAP1"})),
    });
    assert_eq!(add_gap.status, 200);
    assert_eq!(add_gap.body["gap_ids"], json!(["GAP1"]));

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["gap_ids"], json!(["GAP1"]));
    assert_eq!(show.body["feature"]["gap_ids"], json!(["GAP1"]));
    assert_eq!(show.body["feature"]["gap_count"], 1);
    assert_eq!(show.body["feature"]["gaps"][0]["id"], "GAP1");

    let remove_gap = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(remove_gap.status, 200);
    assert_eq!(remove_gap.body["gap_ids"], json!([]));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reorders_and_moves_feature_workflow() {
    let temp_root = unique_temp_dir("http-feature-reorder-move");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for gap_id in ["GAP1", "GAP2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/gaps".to_string(),
            body: Some(json!({"gap_id": gap_id})),
        });
    }

    let reorder = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/gaps/GAP2/reorder".to_string(),
        body: Some(json!({"order": 1})),
    });
    assert_eq!(reorder.status, 200);
    assert_eq!(reorder.body["gap_ids"], json!(["GAP2", "GAP1"]));

    let reorder_before = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/gaps/GAP1/reorder".to_string(),
        body: Some(json!({"before": "GAP2"})),
    });
    assert_eq!(reorder_before.status, 200);
    assert_eq!(reorder_before.body["gap_ids"], json!(["GAP1", "GAP2"]));

    let reorder_after = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/gaps/GAP1/reorder".to_string(),
        body: Some(json!({"after": "GAP2"})),
    });
    assert_eq!(reorder_after.status, 200);
    assert_eq!(reorder_after.body["gap_ids"], json!(["GAP2", "GAP1"]));

    let move_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/move".to_string(),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(move_feature.status, 200);
    assert_eq!(move_feature.body["rollup"]["status"], "todo");
    assert!(
        fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_updates_feature_metadata_and_runs_gap_actions() {
    let temp_root = unique_temp_dir("http-feature-gap-actions");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [
        ("GAP1", "Verify Gap"),
        ("GAP2", "Retry Quality"),
        ("GAP3", "Retry Merge"),
        ("GAP4", "Submit Merge"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Original Feature"})),
    });

    let feature = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/features/FEA1".to_string(),
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
        body: Some(json!({
            "selected_ids": ["GAP1"],
            "update": {"status": "review"}
        })),
    });
    let verified = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/verify".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(verified.status, 200);
    assert_eq!(verified.body["ok"], true);
    assert_eq!(verified.body["gap"]["status"], "done");

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GAP2", "GAP3"],
            "update": {"status": "failed"}
        })),
    });
    let retry_quality = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP2/retry-quality".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(retry_quality.status, 200);
    assert_eq!(retry_quality.body["gap"]["status"], "qa");

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP4/start".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(started.status, 200);
    assert_eq!(started.body["gap"]["status"], "in-progress");
    let submitted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP4/submit-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(submitted.status, 200);
    assert_eq!(submitted.body["gap"]["status"], "ready-merge");
    let submitted_again = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP4/submit-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(submitted_again.status, 200);
    assert_eq!(submitted_again.body["gap"]["status"], "ready-merge");

    let retry_merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/retry-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(retry_merge.status, 200);
    assert_eq!(retry_merge.body["gap"]["status"], "ready-merge");

    let merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(merge.status, 200);
    assert_eq!(merge.body["gap"]["status"], "done");

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/undo".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(undo.status, 200);
    assert_eq!(undo.body["gap"]["status"], "review");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_cancels_and_deletes_features() {
    let temp_root = unique_temp_dir("http-feature-cancel-delete");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for gap_id in ["GAP1", "GAP2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/gaps".to_string(),
            body: Some(json!({"gap_id": gap_id})),
        });
    }

    let gap_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/gaps/GAP1/cancel".to_string(),
        body: None,
    });
    assert_eq!(gap_cancel.status, 200);
    assert_eq!(gap_cancel.body["gap"]["status"], "cancelled");

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .register(ManagedProcess {
            id: "agent-gap2".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
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
    let operation = FileOperationRegistry::new(&runtime_root)
        .register("feature FEA1 gap GAP2")
        .unwrap();

    let feature_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/cancel".to_string(),
        body: None,
    });
    assert_eq!(feature_cancel.status, 200);
    assert_eq!(feature_cancel.body["rollup"]["cancelled_count"], 2);
    assert_eq!(feature_cancel.body["runtime_reconciled"]["processes"], 1);
    assert_eq!(feature_cancel.body["runtime_reconciled"]["operations"], 1);
    assert!(supervisor.inspect(&process.id).is_err());
    assert_eq!(
        FileOperationRegistry::new(&runtime_root)
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Cancelled
    );

    let feature_delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(feature_delete.status, 200);
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("gaps/GA/P1/gap.json").exists());
    assert!(!refine_dir.join("gaps/GA/P2/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_accepts_static_ui_api_aliases_for_work_routes() {
    let temp_root = unique_temp_dir("http-api-aliases");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let create_gap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Gap One"})),
    });
    assert_eq!(create_gap.status, 201);
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);

    let add_gap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(add_gap.status, 200);
    assert_eq!(add_gap.body["gap_ids"], json!(["GAP1"]));

    let workflow = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/workflow".to_string(),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(workflow.status, 200);
    assert_eq!(workflow.body["rollup"]["status"], "todo");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/cancel".to_string(),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["gap"]["status"], "cancelled");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_accepts_static_ui_bulk_api_aliases() {
    let temp_root = unique_temp_dir("http-bulk-api-aliases");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [("GAP1", "Bulk One"), ("GAP2", "Bulk Two")] {
        let create = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
        assert_eq!(create.status, 201);
    }
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Bulk Feature"})),
    });
    assert_eq!(create_feature.status, 201);

    let bulk_status = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
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
        body: Some(json!({"selected_ids": ["GAP1", "GAP2"]})),
    });
    assert_eq!(bulk_assign.status, 200);
    assert_eq!(bulk_assign.body["updated"], 2);
    assert_eq!(
        fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"feature_id\": \"FEA1\""),
        true
    );

    let bulk_delete = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk/delete".to_string(),
        body: Some(json!({"selected_ids": ["GAP1"]})),
    });
    assert_eq!(bulk_delete.status, 200);
    assert_eq!(bulk_delete.body["deleted"], 1);
    assert!(!refine_dir.join("gaps/GA/P1/gap.json").exists());
    assert!(refine_dir.join("gaps/GA/P2/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_records_and_lists_activity_for_static_ui() {
    let temp_root = unique_temp_dir("http-activity");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let recorded = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/ui-error".to_string(),
        body: Some(json!({"message": "Boom", "source": "test"})),
    });
    assert_eq!(recorded.status, 200);
    assert_eq!(recorded.body["recorded"], true);
    assert!(refine_dir.join("logs/activity.jsonl").exists());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["activity"][0]["message"], "Boom");
    assert_eq!(listed.body["facets"]["categories"], json!(["ui"]));

    let filtered = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity?q=source&limit=1".to_string(),
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
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let parsed = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/import/csv/parse".to_string(),
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
fn daemon_agent_automation_loop_executes_todo_gaps_without_manual_request() {
    let temp_root = unique_temp_dir("daemon-agent-automation-loop");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '\\n# automated by smoke-ai loop\\n' >> app.py\nprintf '%s\\n' 'smoke-ai loop response'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _smoke_ai_env_guard = smoke_ai_env_lock().lock().unwrap();
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Loop schedulable"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/transition".to_string(),
        body: Some(json!({"status": "todo"})),
    });

    let daemon = LocalHttpDaemon {
        server: server.clone(),
        static_root: None,
    };
    let automation_loop = daemon.start_agent_automation_loop(Duration::from_millis(25));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let show = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/gaps/GAP1".to_string(),
            body: None,
        });
        assert_eq!(show.status, 200);
        if show.body["gap"]["status"] == "review" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "automation loop did not execute GAP1 before timeout: {}",
            show.body["gap"]["status"]
        );
        thread::sleep(Duration::from_millis(25));
    }
    automation_loop.stop_for_test();

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    assert!(state.contains("\"gap_id\": \"GAP1\""));
    assert!(
        !fs::read_to_string(runtime_root.join(API_EVENTS_FILE))
            .unwrap_or_default()
            .contains("/workflow/")
    );

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_cancels_background_import_persist_and_rolls_back_created_gaps() {
    let temp_root = unique_temp_dir("http-import-cancel");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let prefix = format!(
        "cancel-import-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let drafts = (1..=240)
        .map(|index| {
            json!({
                "name": format!("{prefix}-{index:03}"),
                "actual": format!("{prefix} actual {index:03}"),
                "target": format!("{prefix} target {index:03}"),
                "reporter": "QA",
                "priority": "medium"
            })
        })
        .collect::<Vec<_>>();

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/persist".to_string(),
        body: Some(json!({
            "background": true,
            "drafts": drafts
        })),
    });
    assert_eq!(started.status, 202);
    let operation_id = started.body["operation"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{operation_id}/cancel"),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["operation"]["status"], "cancelled");

    let registry = FileOperationRegistry::new(&runtime_root);
    let worker_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let operation = registry.status(&operation_id).unwrap();
        if operation.progress["message"] == "Import cancelled" {
            assert_eq!(operation.state, OperationState::Cancelled);
            assert_eq!(operation.progress["completed"], 0);
            assert_eq!(operation.progress["total"], 240);
            break;
        }
        assert!(
            !matches!(
                operation.state,
                OperationState::Succeeded | OperationState::Failed
            ),
            "background import finished instead of observing cancellation: {:?}",
            operation
        );
        assert!(
            Instant::now() < worker_deadline,
            "timed out waiting for background import worker to observe cancellation: {:?}",
            operation
        );
        thread::sleep(Duration::from_millis(10));
    }

    let projection_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let gaps = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/gaps?limit=1000&node=current&q={prefix}"),
            body: None,
        });
        assert_eq!(gaps.status, 200);
        let total = gaps.body["page"]["total"].as_u64().unwrap();
        if total == 0 {
            break;
        }
        assert!(
            Instant::now() < projection_deadline,
            "cancelled import left {total} matching Gap records"
        );
        thread::sleep(Duration::from_millis(10));
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_rebuilds_projection_cache_and_serves_changes_performance_routes() {
    let temp_root = unique_temp_dir("http-cache-changes-performance");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Cached Gap"})),
    });
    let rebuilt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cache/rebuild".to_string(),
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
        body: None,
    });
    assert_eq!(changes.status, 200);
    assert_eq!(changes.body["branch"], serde_json::Value::Null);
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
    let cached = FileProjectStateStore::new(&refine_dir)
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
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let gap_dir = refine_dir.join("gaps").join("GA").join("P1");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&gap_dir).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "one\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "Change-linked Gap",
              "status": "todo",
              "priority": "high",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();
    fs::write(temp_root.join("app.txt"), "unrelated\n").unwrap();
    git(&temp_root, &["commit", "-am", "maintenance update"]).unwrap();
    fs::write(temp_root.join("app.txt"), "two\n").unwrap();
    git(&temp_root, &["commit", "-am", "GAP1 update app"]).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let changes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/changes?limit=5".to_string(),
        body: None,
    });
    assert_eq!(changes.status, 200);
    assert_eq!(changes.body["page"]["total"], 1);
    assert_eq!(changes.body["changes"][0]["subject"], "GAP1 update app");
    assert_eq!(changes.body["changes"][0]["gap_id"], "GAP1");
    let commit = changes.body["changes"][0]["commit"].as_str().unwrap();

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/changes/undo".to_string(),
        body: Some(json!({"commit": commit})),
    });
    assert_eq!(undo.status, 200);
    assert_eq!(undo.body["ok"], true);
    assert_eq!(
        fs::read_to_string(temp_root.join("app.txt")).unwrap(),
        "unrelated\n"
    );
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_hard_resets_git_worktree() {
    let temp_root = unique_temp_dir("http-git-reset");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "committed\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(temp_root.join("app.txt"), "dirty\n").unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let reset = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
        body: None,
    });
    assert_eq!(reset.status, 200);
    assert_eq!(reset.body["ok"], true);
    assert_eq!(
        fs::read_to_string(temp_root.join("app.txt")).unwrap(),
        "committed\n"
    );
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_reports_no_git_repo_and_missing_upstream() {
    let temp_root = unique_temp_dir("http-project-sync-basic");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    let no_repo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(no_repo.status, 200);
    assert_eq!(no_repo.body["git_sync"]["attempted"], false);
    assert_eq!(no_repo.body["git_sync"]["pulled"], false);
    assert!(no_repo.body["git_sync"]["detail"].is_null());

    git(&app_root, &["init", "-b", "main"]).unwrap();
    git(&app_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&app_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(app_root.join("app.txt"), "initial\n").unwrap();
    git(&app_root, &["add", "app.txt"]).unwrap();
    git(&app_root, &["commit", "-m", "initial"]).unwrap();
    let missing_upstream = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(missing_upstream.status, 200);
    assert_eq!(missing_upstream.body["git_sync"]["attempted"], false);
    assert_eq!(
        missing_upstream.body["git_sync"]["detail"],
        "No upstream branch configured."
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_pulls_fast_forward_and_allows_refine_runtime_noise() {
    let temp_root = unique_temp_dir("http-project-sync-ff");
    let remote = temp_root.join("remote.git");
    let seed = temp_root.join("seed");
    let app_root = temp_root.join("app");
    fs::create_dir_all(&temp_root).unwrap();
    git(
        &temp_root,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .unwrap();
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-b", "main"]).unwrap();
    git(&seed, &["config", "user.email", "test@example.com"]).unwrap();
    git(&seed, &["config", "user.name", "Test User"]).unwrap();
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .unwrap();
    fs::write(seed.join("app.txt"), "initial\n").unwrap();
    git(&seed, &["add", "app.txt"]).unwrap();
    git(&seed, &["commit", "-m", "initial"]).unwrap();
    git(&seed, &["push", "-u", "origin", "main"]).unwrap();
    git(
        &temp_root,
        &[
            "clone",
            remote.to_str().unwrap(),
            app_root.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::create_dir_all(app_root.join(".refine/runtime/processes")).unwrap();
    fs::write(
        app_root.join(".refine/runtime/processes/local.json"),
        r#"{"id":"local","owner":"maintenance","pid":null,"state":"running","label":"local","details":"runtime noise","started_at":"now"}"#,
    )
    .unwrap();
    fs::write(seed.join("remote.txt"), "remote\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "remote update"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(temp_root.join("run/8080"));
    let sync = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(sync.status, 200);
    assert_eq!(sync.body["git_sync"]["attempted"], true);
    assert_eq!(sync.body["git_sync"]["pulled"], true);
    assert!(app_root.join("remote.txt").exists());
    assert!(
        app_root
            .join(".refine/runtime/processes/local.json")
            .exists()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_skips_pull_for_dirty_user_worktree() {
    let temp_root = unique_temp_dir("http-project-sync-dirty");
    let (seed, app_root) = seeded_remote_clone(&temp_root);
    fs::write(seed.join("remote.txt"), "remote\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "remote update"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();
    fs::write(app_root.join("local.txt"), "local dirty\n").unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(temp_root.join("run/8080"));
    let sync = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(sync.status, 200);
    assert_eq!(sync.body["git_sync"]["attempted"], false);
    assert_eq!(
        sync.body["git_sync"]["detail"],
        "Local worktree changes present; skipped upstream pull."
    );
    assert!(!app_root.join("remote.txt").exists());
    assert_eq!(
        fs::read_to_string(app_root.join("local.txt")).unwrap(),
        "local dirty\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_reports_pull_failure_for_diverged_branch() {
    let temp_root = unique_temp_dir("http-project-sync-diverged");
    let (seed, app_root) = seeded_remote_clone(&temp_root);
    git(&app_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&app_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(seed.join("remote.txt"), "remote\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "remote update"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();
    fs::write(app_root.join("local.txt"), "local\n").unwrap();
    git(&app_root, &["add", "local.txt"]).unwrap();
    git(&app_root, &["commit", "-m", "local update"]).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(temp_root.join("run/8080"));
    let sync = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    assert_ne!(sync.status, 200);
    assert!(
        sync.body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("failed to sync project state from upstream")
    );
    assert!(!app_root.join("remote.txt").exists());
    assert!(app_root.join("local.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_cleans_activity_and_reports_unconnected_native_actions() {
    let temp_root = unique_temp_dir("http-cleanups");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/ui-error".to_string(),
        body: Some(json!({"message": "Boom"})),
    });
    assert!(refine_dir.join("logs/activity.jsonl").exists());
    let cleanup = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/cleanup".to_string(),
        body: Some(json!({"days": 0})),
    });
    assert_eq!(cleanup.status, 200);
    assert_eq!(cleanup.body["deleted"], 1);
    assert!(!refine_dir.join("logs/activity.jsonl").exists());

    let runtime_root = temp_root.join("run/8080");
    server.runtime_root = Some(runtime_root.clone());
    let metrics = FileMetricsService::new(&runtime_root);
    metrics
        .record_operation("old", 10.0, true, json!({}))
        .unwrap();
    let performance_before_cleanup = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/performance".to_string(),
        body: None,
    });
    assert_eq!(performance_before_cleanup.status, 200);
    assert_eq!(performance_before_cleanup.body["total_event_count"], 1);
    let performance_cleanup = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/performance/cleanup".to_string(),
        body: Some(json!({"clear": true})),
    });
    assert_eq!(performance_cleanup.status, 200);
    assert_eq!(performance_cleanup.body["deleted"], 1);
    assert!(!metrics.path().exists());
    let performance_after_cleanup = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/performance".to_string(),
        body: None,
    });
    assert_eq!(performance_after_cleanup.status, 200);
    assert_eq!(performance_after_cleanup.body["total_event_count"], 0);

    let undo = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/changes/undo".to_string(),
        body: Some(json!({"commit": "abc123"})),
    });
    assert_eq!(undo.status, 200);
    assert_eq!(undo.body["ok"], false);

    let reset = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
        body: None,
    });
    assert_eq!(reset.status, 200);
    assert_eq!(reset.body["ok"], false);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_nodes_and_transfers_gap_ownership() {
    let temp_root = unique_temp_dir("http-node-transfer");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [
        ("GAP1", "Transfer One"),
        ("GAP2", "Transfer Two"),
        ("GAP3", "Stay Default"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
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
        body: Some(json!({"node_id": "remote-qa"})),
    });
    assert_eq!(activated.status, 200);
    assert_eq!(activated.body["active_node_id"], "remote-qa");
    assert!(refine_dir.join("active-node.json").exists());

    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-gaps".to_string(),
        body: Some(json!({
            "selected_ids": ["GAP1", "GAP2"],
            "target_node_id": "remote-qa"
        })),
    });
    assert_eq!(transfer.status, 200);
    assert_eq!(transfer.body["updated"], 2);
    let current_node_gaps = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps?node=current".to_string(),
        body: None,
    });
    assert_eq!(current_node_gaps.status, 200);
    assert_eq!(current_node_gaps.body["page"]["total"], 2);
    assert_eq!(
        current_node_gaps.body["gaps"][0]["node_display_name"],
        "Remote QA"
    );
    let all_node_gaps = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_node_gaps.status, 200);
    assert_eq!(all_node_gaps.body["page"]["total"], 3);
    let gap = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(gap.body["gap"]["node_id"], "remote-qa");

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/nodes/remote-qa".to_string(),
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
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cluster/nodes".to_string(),
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
    assert!(refine_dir.join("cluster.json").exists());

    let disabled = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/cluster/nodes/node-1".to_string(),
        body: Some(json!({"enabled": false, "ssh_port": 2222})),
    });
    assert_eq!(disabled.status, 200);
    assert_eq!(disabled.body["nodes"][0]["enabled"], false);
    assert_eq!(disabled.body["nodes"][0]["ssh_port"], 2222);

    let bootstrap = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cluster/nodes/node-1/bootstrap".to_string(),
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
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(temp_root.join("src")).unwrap();
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(temp_root.join("README.md"), "hello\nworld\n").unwrap();
    fs::write(temp_root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        temp_root.join("pixel.png"),
        [
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0x00, 0x00, 0x00, 0x00,
        ],
    )
    .unwrap();
    fs::write(temp_root.join("artifact.bin"), [0x00, 0x01, 0x02]).unwrap();
    fs::write(refine_dir.join("settings.json"), "{}").unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let tree = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/tree?path=&recursive=1&max_depth=2&max_entries=20".to_string(),
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
    let src_index = root_entries
        .iter()
        .position(|entry| entry["path"] == "src")
        .unwrap();
    let readme_index = root_entries
        .iter()
        .position(|entry| entry["path"] == "README.md")
        .unwrap();
    assert!(src_index < readme_index);
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
        body: None,
    });
    assert_eq!(read.status, 200);
    assert_eq!(read.body["previewable"], true);
    assert_eq!(read.body["content"], "hello\n");
    assert_eq!(read.body["has_more"], true);
    assert_eq!(read.body["next_offset"], 6);

    let image = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=pixel.png".to_string(),
        body: None,
    });
    assert_eq!(image.status, 200);
    assert_eq!(image.body["previewable"], true);
    assert_eq!(image.body["kind"], "image");
    assert_eq!(image.body["mime_type"], "image/png");
    assert!(
        image.body["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );

    let binary = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=artifact.bin".to_string(),
        body: None,
    });
    assert_eq!(binary.status, 200);
    assert_eq!(binary.body["previewable"], false);
    assert_eq!(binary.body["kind"], "binary");

    let search = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/search?q=main&max_entries=5".to_string(),
        body: None,
    });
    assert_eq!(search.status, 200);
    assert_eq!(search.body["entries"][0]["path"], "src/main.rs");

    let traversal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=../Cargo.toml".to_string(),
        body: None,
    });
    assert_eq!(traversal.status, 400);
    assert_eq!(traversal.body["error"]["code"], "invalid_input");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_runs_interactive_terminal_session() {
    let temp_root = unique_temp_dir("http-terminal");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(temp_root.join("README.md"), "terminal root\n").unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let start = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"cols": 80, "rows": 20})),
    });
    assert_eq!(start.status, 200);
    assert_eq!(start.body["cwd"], temp_root.display().to_string());
    let session_id = start.body["id"].as_str().unwrap().to_string();

    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/input"),
        body: Some(json!({"data": "printf 'terminal:%s' \"$(cat README.md)\"\r"})),
    });
    assert_eq!(input.status, 200);

    let mut output = String::new();
    for _ in 0..40 {
        let events = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/terminal/{session_id}/events"),
            body: None,
        });
        assert_eq!(events.status, 200);
        for event in events.body["events"].as_array().unwrap() {
            output.push_str(event["data"].as_str().unwrap_or(""));
        }
        if output.contains("terminal:terminal root") {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(output.contains("terminal:terminal root"), "{output:?}");

    let stop = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stop.status, 200);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_serves_project_utility_upgrade_health_and_sse_routes() {
    let temp_root = unique_temp_dir("http-project-utils");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(temp_root.join("child")).unwrap();
    init_git_app(&temp_root);
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let path = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!(
            "/api/project/path?path={}",
            percent_encode_for_test(temp_root.to_str().unwrap())
        ),
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
        body: None,
    });
    assert_eq!(upgrade.status, 200);
    assert_eq!(upgrade.body["upgrade"]["available"], false);
    assert_eq!(upgrade.body["upgrade"]["upgrade_available"], false);
    assert_eq!(upgrade.body["upgrade"]["local_development"], true);

    let install = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/install".to_string(),
        body: Some(json!({"target": "linux-cli-web", "version": "1.0.0"})),
    });
    assert_eq!(install.status, 200);
    assert_eq!(install.body["install"]["installed"], true);
    assert_eq!(install.body["install"]["target"], "linux_cli_web");

    let install_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/system/install".to_string(),
        body: None,
    });
    assert_eq!(install_status.status, 200);
    assert_eq!(install_status.body["install"]["installed"], true);

    let update = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/update".to_string(),
        body: Some(json!({"version": "1.1.0"})),
    });
    assert_eq!(update.status, 501);
    assert!(
        update.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("./r system update")
    );

    let install_status_after_update = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/system/install".to_string(),
        body: None,
    });
    assert_eq!(install_status_after_update.status, 200);
    assert_eq!(
        install_status_after_update.body["install"]["version"],
        "1.0.0"
    );

    let uninstall = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/uninstall".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(uninstall.status, 200);
    assert_eq!(uninstall.body["uninstalled"], true);

    let health = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/health".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(health.status, 200);
    assert_eq!(health.body["last_check_ok"], true);

    let operation_registry = FileOperationRegistry::new(&runtime_root);
    let operation = operation_registry.register("sse-operation").unwrap();
    operation_registry
        .append_log(
            &operation.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "SSE operation progress".to_string(),
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
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
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
    let chat = FileChatService::new(&refine_dir);
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
    assert!(sse_body.contains("event: operation_progress"));
    assert!(sse_body.contains("SSE operation progress"));
    assert!(sse_body.contains("event: chat_event"));
    assert!(sse_body.contains("SSE chat event"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reads_and_cancels_runtime_operations() {
    let temp_root = unique_temp_dir("http-operations");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry.register("bulk_update_gaps").unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{}", operation.id),
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["operation"]["status"], "running");
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(cached.runtime.background_operations[0]["id"], operation.id);
    assert_eq!(cached.runtime.background_operations[0]["status"], "running");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{}/cancel", operation.id),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["operation"]["status"], "cancelled");
    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{}/logs?limit=10", operation.id),
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["total"], 2);
    assert!(
        logs.body["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["message"] == "Operation cancelled")
    );
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.runtime.background_operations[0]["status"],
        "cancelled"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_retries_workflow_executions() {
    let temp_root = unique_temp_dir("http-workflow-execution-retry");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let automation = WorkflowEngine::new(&runtime_root);
    let claim_id = automation.claim("GAP1").unwrap();
    let execution_id = automation.start_claim(&claim_id).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let retry = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/workflow/executions/{execution_id}/retry"),
        body: None,
    });
    assert_eq!(retry.status, 200);
    assert_eq!(retry.body["retried_from"], execution_id);
    assert_eq!(retry.body["execution"]["gap_id"], "GAP1");
    assert_eq!(retry.body["execution"]["status"], "running");
    assert_ne!(retry.body["execution"]["id"], execution_id);

    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert!(cached.runtime.background_operations.is_empty());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_lists_processes_and_updates_pause_controls() {
    let temp_root = unique_temp_dir("http-processes");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
    let standalone_chat = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), None)
        .unwrap();
    let gap_chat = chat
        .start_with_options(
            ChatAttachment::Gap("GAPCHAT".to_string()),
            Some("smoke-ai"),
            Some("gap"),
        )
        .unwrap();
    let stopped_chat = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), None)
        .unwrap();
    chat.stop(&stopped_chat.id).unwrap();
    supervisor
        .launch(crate::process::subprocess::ManagedProcessSpec {
            owner: crate::process::subprocess::ProcessOwner::Agent,
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
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "agent-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Agent context".to_string()),
            details: Some(json!({"gap_id": "GAPCTX", "round_idx": 1}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "chat-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Chat context".to_string()),
            details: Some(
                json!({"session_id": "chat-context-session", "mode": "standalone"}).to_string(),
            ),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "ui-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("UI context".to_string()),
            details: Some(json!({"kind": "ui"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "exited-agent-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: None,
            state: "exited".to_string(),
            label: Some("Exited agent context".to_string()),
            details: Some(json!({"gap_id": "DONECTX", "round_idx": 1}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    fs::write(runtime_root.join("processes/empty-process.json"), "").unwrap();
    fs::write(
        runtime_root.join("processes/malformed-process.json"),
        "{not json",
    )
    .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["processes"][0]["kind"], "agent");
    assert_eq!(listed.body["runner_reachable"], true);
    assert_eq!(
        listed.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .map(|work| work["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "merger",
            "plan_draft_extractor",
            "target_app_builder",
            "target_app_config_generator",
            "sqlite_cache_rebuild",
            "activity_log_cleanup"
        ]
    );
    assert!(
        listed.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|work| work["status"] == "idle")
    );
    let listed_processes = listed.body["processes"].as_array().unwrap();
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "exited-agent-context")
    );
    let agent_context = listed_processes
        .iter()
        .find(|process| process["id"] == "agent-context")
        .unwrap();
    assert_eq!(agent_context["gap_id"], "GAPCTX");
    assert_eq!(agent_context["round_idx"], 1);
    let chat_context = listed_processes
        .iter()
        .find(|process| process["id"] == "chat-context")
        .unwrap();
    assert_eq!(chat_context["kind"], "chat");
    assert_eq!(chat_context["session_id"], "chat-context-session");
    assert_eq!(chat_context["mode"], "standalone");
    let standalone_context = listed_processes
        .iter()
        .find(|process| process["id"] == format!("chat-session-{}", standalone_chat.id))
        .unwrap();
    assert_eq!(standalone_context["kind"], "chat");
    assert_eq!(standalone_context["session_id"], standalone_chat.id);
    assert_eq!(standalone_context["mode"], "standalone");
    let gap_chat_context = listed_processes
        .iter()
        .find(|process| process["id"] == format!("chat-session-{}", gap_chat.id))
        .unwrap();
    assert_eq!(gap_chat_context["kind"], "chat");
    assert_eq!(gap_chat_context["session_id"], gap_chat.id);
    assert_eq!(gap_chat_context["mode"], "gap");
    assert_eq!(gap_chat_context["gap_id"], "GAPCHAT");
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == format!("chat-session-{}", stopped_chat.id))
    );
    let ui_context = listed_processes
        .iter()
        .find(|process| process["id"] == "ui-context")
        .unwrap();
    assert_eq!(ui_context["kind"], "ui");
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "empty-process")
    );
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "malformed-process")
    );

    supervisor
        .register(ManagedProcess {
            id: "exited-target-context".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: None,
            state: "exited".to_string(),
            label: Some("sh".to_string()),
            details: Some("-c old-app-status".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "dead-target-context".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: Some(99_999_999),
            state: "running".to_string(),
            label: Some("sh".to_string()),
            details: Some("-c stale-app-status".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let listed_after_target_records = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        body: None,
    });
    assert_eq!(listed_after_target_records.status, 200);
    let listed_after_target_records = listed_after_target_records.body["processes"]
        .as_array()
        .unwrap();
    assert!(
        !listed_after_target_records
            .iter()
            .any(|process| process["id"] == "exited-target-context")
    );
    assert!(
        !listed_after_target_records
            .iter()
            .any(|process| process["id"] == "dead-target-context")
    );
    assert!(supervisor.inspect("dead-target-context").is_err());

    let summary = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes?summary=1".to_string(),
        body: None,
    });
    assert_eq!(summary.status, 200);
    assert_eq!(summary.body["agent_count"], 2);
    assert_eq!(summary.body["process_count"], 6);
    assert_eq!(summary.body["processes"].as_array().unwrap().len(), 0);
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert!(
        cached
            .runtime
            .processes
            .iter()
            .any(|process| process["gap_id"] == "GAPCTX")
    );
    assert_eq!(
        cached.runtime.supervisor.unwrap()["runner_reachable"],
        json!(true)
    );

    let stdout_path = runtime_root.join("stream.stdout.log");
    let stderr_path = runtime_root.join("stream.stderr.log");
    fs::write(&stdout_path, "hello stdout\n").unwrap();
    fs::write(&stderr_path, "warn stderr\n").unwrap();
    supervisor
        .register(crate::process::subprocess::ManagedProcess {
            id: "stream-test".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
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

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_gap_summary("Stop background rollback", Some("GAP-BACKGROUND"))
        .unwrap();
    work_items
        .transition_gap_status("GAP-BACKGROUND", GapStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_gap_status("GAP-BACKGROUND", GapStatus::InProgress)
        .unwrap();
    let background = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/background".to_string(),
        body: Some(json!({"stopped": true})),
    });
    assert_eq!(background.status, 200);
    assert_eq!(background.body["background_processes_stopped"], true);
    assert_eq!(
        work_items
            .show_gap_summary("GAP-BACKGROUND")
            .unwrap()
            .gap
            .status,
        GapStatus::Todo
    );
    assert!(
        background.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|work| work["status"] == "paused")
    );

    work_items
        .create_gap_summary("Pause agents rollback", Some("GAP-AGENTS"))
        .unwrap();
    work_items
        .transition_gap_status("GAP-AGENTS", GapStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_gap_status("GAP-AGENTS", GapStatus::InProgress)
        .unwrap();
    let agents = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/agents".to_string(),
        body: Some(json!({"paused": true})),
    });
    assert_eq!(agents.status, 200);
    assert_eq!(agents.body["agents_paused"], true);
    assert_eq!(
        work_items
            .show_gap_summary("GAP-AGENTS")
            .unwrap()
            .gap
            .status,
        GapStatus::Todo
    );
    assert!(runtime_root.join("process-control.json").exists());
    let cached = FileProjectStateStore::new(&refine_dir)
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
        body: None,
    });
    assert_eq!(agents.status, 200);
    assert!(agents.body["providers"].as_array().unwrap().len() >= 5);
    assert_eq!(agents.body["stage"], "provider_detection");

    let diagnostics = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/smoke-ai/diagnostics".to_string(),
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
        body: None,
    });
    assert_eq!(configured.status, 200);
    assert_eq!(configured.body["configured"], true);

    let auth = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/agents/smoke-ai/auth".to_string(),
        body: None,
    });
    assert!(auth.status == 200 || auth.status == 503);
    if auth.status == 503 {
        assert_eq!(auth.body["error"]["code"], "degraded");
    }

    let invalid = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/not-a-provider/diagnostics".to_string(),
        body: None,
    });
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"]["code"], "invalid_input");

    let recheck = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/settings/recheck-auth".to_string(),
        body: None,
    });
    assert_eq!(recheck.status, 200);
    assert!(recheck.body["message"].as_str().unwrap().contains("CLI"));
}

#[test]
fn web_server_manages_quality_settings_and_regressions() {
    let temp_root = unique_temp_dir("http-quality");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    write_fake_playwright(&temp_root, 0);
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let app_settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({"target_app_url": "http://127.0.0.1:3000"})),
    });
    assert_eq!(app_settings.status, 200);

    let initial = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        body: None,
    });
    assert_eq!(initial.status, 200);
    assert_eq!(initial.body["enabled"], "0");
    assert_eq!(initial.body["timing"], "pre_merge");

    let saved = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/quality".to_string(),
        body: Some(json!({
            "enabled": "1",
            "timing": "post_build",
            "regressions_enabled": true,
            "business_requirements": "Dashboard must render",
            "instructions": "Run focused checks"
        })),
    });
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["enabled"], "1");
    assert_eq!(saved.body["timing"], "post_build");
    assert_eq!(saved.body["regressions_enabled"], "1");
    assert_eq!(saved.body["configured"], true);

    let checks = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/checks".to_string(),
        body: Some(json!({
            "owner_id": "GAP1",
            "command": "printf quality-ok"
        })),
    });
    assert_eq!(checks.status, 200);
    assert_eq!(checks.body["ok"], true);
    assert_eq!(checks.body["result"]["owner_id"], "GAP1");
    assert_eq!(checks.body["operation"]["owner"], "quality:GAP1");
    assert_eq!(checks.body["operation"]["status"], "complete");
    assert!(
        checks.body["result"]["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("quality-ok")
    );
    let quality_operation_id = checks.body["operation"]["id"].as_str().unwrap();
    let quality_operation_logs = FileOperationRegistry::new(&runtime_root)
        .page_logs(quality_operation_id, 10, 0)
        .unwrap()
        .0;
    assert!(
        quality_operation_logs
            .iter()
            .any(|log| log.message == "Quality checks passed")
    );

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/regressions".to_string(),
        body: Some(json!({
            "title": "Dashboard smoke",
            "prompt": "Open the dashboard",
            "description": "Dashboard scenario"
        })),
    });
    assert_eq!(created.status, 201);
    assert_eq!(created.body["regression"]["id"], "dashboard-smoke");
    assert!(
        refine_dir
            .join("regressions/specs/dashboard-smoke.spec.cjs")
            .exists()
    );

    let run = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/regressions/run".to_string(),
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
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["regressions"][0]["latest_run"]["ok"], true);

    let disabled = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/quality/regressions/dashboard-smoke".to_string(),
        body: Some(json!({"enabled": false})),
    });
    assert_eq!(disabled.status, 200);
    assert_eq!(disabled.body["regression"]["enabled"], false);

    let deleted = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/quality/regressions/dashboard-smoke".to_string(),
        body: None,
    });
    assert_eq!(deleted.status, 200);
    assert_eq!(deleted.body["ok"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_refine_chat_sessions() {
    let temp_root = unique_temp_dir("http-chat");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let _smoke_ai_env_guard = smoke_ai_env_lock().lock().unwrap();
    write_fake_provider(
        &refine_dir,
        "smoke-ai",
        0,
        "{\"message\":\"web provider output\",\"importable_artifacts\":[{\"type\":\"round\",\"round\":{\"reporter\":\"QA\",\"actual\":\"Broken\",\"target\":\"Fixed\"}}]}",
    );
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/chat/start".to_string(),
        body: Some(json!({"gap_id": "GAP1", "provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201);
    let session_id = started.body["session_id"].as_str().unwrap().to_string();
    assert_eq!(started.body["mode"], "gap");
    assert!(
        refine_dir
            .join(format!("chat/sessions/{session_id}.json"))
            .exists()
    );

    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/input"),
        body: Some(json!({"text": "What should I test?"})),
    });
    assert_eq!(input.status, 200);
    assert_eq!(input.body["queued_messages"].as_array().unwrap().len(), 1);

    let read = wait_for_chat_read_line(&server, &session_id, "web provider output");
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
    let operations = FileOperationRegistry::new(&runtime_root).recover().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].owner, format!("chat:{session_id}"));
    assert_eq!(operations[0].state, OperationState::Succeeded);
    let operation = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{}", operations[0].id),
        body: None,
    });
    assert_eq!(operation.status, 200);
    assert_eq!(
        operation.body["operation"]["owner"],
        format!("chat:{session_id}")
    );
    assert_eq!(operation.body["operation"]["status"], "complete");

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stopped.status, 200);
    assert_eq!(stopped.body["alive"], false);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_edits_and_removes_queued_chat_messages() {
    let temp_root = unique_temp_dir("http-chat-queue");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/chat/start".to_string(),
        body: Some(json!({"provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201);
    let session_id = started.body["session_id"].as_str().unwrap().to_string();
    let session_path = refine_dir.join(format!("chat/sessions/{session_id}.json"));
    let mut persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
    persisted["queue_dispatching"] = json!(true);
    fs::write(
        &session_path,
        serde_json::to_string_pretty(&persisted).unwrap(),
    )
    .unwrap();

    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/input"),
        body: Some(json!({"text": "queued text"})),
    });
    assert_eq!(input.status, 200);
    assert_eq!(input.body["in_flight"], true);
    let message_id = input.body["queued_messages"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/chat/{session_id}/queue/{message_id}"),
        body: Some(json!({"text": "edited queued text"})),
    });
    assert_eq!(updated.status, 200);
    assert_eq!(
        updated.body["queued_messages"][0]["text"],
        "edited queued text"
    );
    let removed = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: format!("/api/chat/{session_id}/queue/{message_id}"),
        body: None,
    });
    assert_eq!(removed.status, 200);
    assert_eq!(removed.body["queued_messages"].as_array().unwrap().len(), 0);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_standalone_chat_start_and_stop_manage_worktree() {
    let temp_root = unique_temp_dir("http-chat-standalone-worktree");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/chat/start".to_string(),
        body: Some(json!({"provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201, "{started:#?}");
    let session_id = started.body["session_id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(started.body["worktree"]["path"].as_str().unwrap());
    let branch = started.body["worktree"]["branch"].as_str().unwrap();
    assert!(worktree_path.join(".git").exists());
    assert_eq!(
        git_stdout(&worktree_path, &["branch", "--show-current"]),
        branch
    );

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stopped.status, 200, "{stopped:#?}");
    assert!(!worktree_path.exists());
    assert!(
        git(
            &temp_root,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
        )
        .is_err()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_submit_standalone_chat_creates_ready_merge_gap_and_preserves_worktree() {
    let temp_root = unique_temp_dir("http-chat-standalone-submit");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/chat/start".to_string(),
        body: Some(json!({"provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201, "{started:#?}");
    let session_id = started.body["session_id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(started.body["worktree"]["path"].as_str().unwrap());
    let branch = started.body["worktree"]["branch"]
        .as_str()
        .unwrap()
        .to_string();
    fs::write(
        worktree_path.join("experiment.txt"),
        "standalone experiment\n",
    )
    .unwrap();

    let submitted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/submit-ready-merge"),
        body: Some(json!({
            "reporter": "QA",
            "actual": "Standalone experiment is not merged.",
            "target": "Standalone experiment is ready for the merge workflow.",
            "priority": "medium"
        })),
    });
    assert_eq!(submitted.status, 201, "{submitted:#?}");
    let gap_id = submitted.body["gap"]["id"].as_str().unwrap().to_string();
    assert_eq!(submitted.body["gap"]["status"], "ready-merge");
    assert_eq!(submitted.body["gap"]["branch_name"], branch);
    assert_eq!(submitted.body["gap"]["priority"], "medium");
    assert!(worktree_path.exists());
    assert_eq!(
        git_stdout(&worktree_path, &["rev-list", "--count", "main..HEAD"]),
        "1"
    );

    let session: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(refine_dir.join(format!("chat/sessions/{session_id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(session["closed"], true);
    assert_eq!(session["worktree"]["submitted_gap_id"], gap_id);

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stopped.status, 200, "{stopped:#?}");
    assert!(worktree_path.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn local_http_daemon_recovers_stale_chat_turns_before_serving() {
    let temp_root = unique_temp_dir("http-chat-recovery");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
    let session = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    let operation = FileOperationRegistry::new(&runtime_root)
        .register(&format!("chat:{}", session.id))
        .unwrap();
    let session_path = refine_dir.join(format!("chat/sessions/{}.json", session.id));

    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
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
        FileOperationRegistry::new(&runtime_root)
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Interrupted
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reports_project_registry_and_updates_settings() {
    let temp_root = unique_temp_dir("http-project-settings");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let app_registry_root = temp_root.join("run");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/project/status".to_string(),
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["attached"], true);
    assert_eq!(status.body["target_root"], app_root.display().to_string());
    assert_eq!(status.body["apps"].as_array().unwrap().len(), 1);
    assert!(app_registry_root.join("apps.json").exists());
    assert!(!runtime_root.join("apps.json").exists());

    let app_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/apps/status".to_string(),
        body: None,
    });
    assert_eq!(app_status.status, 200);
    assert_eq!(app_status.body["attached"], true);

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "old-target-app-process".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: None,
            state: "running".to_string(),
            label: Some("sh".to_string()),
            details: Some("-c old target app".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let other_app = temp_root.join("other");
    fs::create_dir_all(&other_app).unwrap();
    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": other_app.display().to_string()})),
    });
    assert_eq!(attached.status, 200);
    assert_eq!(
        attached.body["target_root"],
        other_app.display().to_string()
    );
    assert!(supervisor.inspect("old-target-app-process").is_err());
    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body["attached"], true);

    let switched = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(switched.status, 200);
    assert_eq!(switched.body["target_root"], app_root.display().to_string());

    let third_app = temp_root.join("third");
    fs::create_dir_all(&third_app).unwrap();
    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/register".to_string(),
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
        body: Some(json!({"name": "third-app"})),
    });
    assert_eq!(switched_by_name.status, 200);
    assert_eq!(
        switched_by_name.body["target_root"],
        third_app.display().to_string()
    );

    let detached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/detach".to_string(),
        body: None,
    });
    assert_eq!(detached.status, 200);
    assert_eq!(detached.body["attached"], false);
    assert_eq!(detached.body["target_root"], serde_json::Value::Null);

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/apps".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["apps"].as_array().unwrap().len(), 4);
    assert_eq!(listed.body["current"], "");

    let settings = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/settings".to_string(),
        body: None,
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "claude");
    assert_eq!(settings.body["runtime"]["paused"], false);

    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
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
    assert!(refine_dir.join("settings.json").exists());

    let removed = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/apps".to_string(),
        body: Some(json!({"path": other_app.display().to_string()})),
    });
    assert_eq!(removed.status, 200);
    assert_eq!(removed.body["apps"].as_array().unwrap().len(), 3);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_attach_creates_missing_local_project() {
    let temp_root = unique_temp_dir("http-project-create-local");
    let destination = temp_root.join("new-app");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());

    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": destination.display().to_string()})),
    });

    assert_eq!(attached.status, 200);
    assert_eq!(
        attached.body["target_root"],
        destination.display().to_string()
    );
    assert!(destination.join(".git").exists());
    assert!(destination.join(".refine/refine.json").exists());
    assert!(!runtime_root.join("processes").exists());
    assert!(!destination.join(".refine/runtime/processes").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_applies_runtime_settings_updates_immediately() {
    let temp_root = unique_temp_dir("http-runtime-settings-apply");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Instant runtime settings"})),
    });
    assert_eq!(created.status, 201);

    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "parallel_run_cap": 6,
            "parallel_per_node_cap": 6,
            "backlog_promote_after_seconds": "0"
        })),
    });
    assert_eq!(updated.status, 200);
    assert_eq!(updated.body["settings"]["parallel_run_cap"], "6");
    assert_eq!(
        updated.body["settings"]["backlog_promote_after_seconds"],
        "0"
    );

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["policy"]["global_limit"], 6);
    assert_eq!(state["policy"]["per_node_limit"], 6);
    assert_eq!(state["claims"].as_array().unwrap().len(), 0);

    let gap = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/gaps/GAP1".to_string(),
        body: None,
    });
    assert_eq!(gap.status, 200);
    assert_eq!(gap.body["gap"]["status"], "todo");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_migrates_legacy_project_state_automatically() {
    let temp_root = unique_temp_dir("http-project-migration");
    let runtime_root = temp_root.join("run/8080");
    let app_root = temp_root.join("legacy-app");
    let refine_dir = app_root.join(".refine");
    fs::create_dir_all(refine_dir.join("gaps/GA")).unwrap();
    fs::write(refine_dir.join("gaps/GA/gap.json"), "{}").unwrap();

    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());

    let migrated_attach = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(migrated_attach.status, 200);
    assert_eq!(migrated_attach.body["schema"]["compatible"], true);
    assert_eq!(migrated_attach.body["schema"]["schema_version"], 1);
    assert!(refine_dir.join("refine.json").exists());
    assert!(refine_dir.join("backups/migrations").exists());

    let migrate_again = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/migrate".to_string(),
        body: None,
    });
    assert_eq!(migrate_again.status, 200);
    assert_eq!(migrate_again.body["ok"], true);
    assert_eq!(migrate_again.body["migrated"], false);
    assert_eq!(migrate_again.body["schema"]["compatible"], true);

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/project/status".to_string(),
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["schema"]["migration_required"], false);

    let second_app = temp_root.join("second-legacy-app");
    let second_refine_dir = second_app.join(".refine");
    fs::create_dir_all(second_refine_dir.join("features")).unwrap();
    let registered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/register".to_string(),
        body: Some(json!({
            "name": "second",
            "path": second_app.display().to_string()
        })),
    });
    assert_eq!(registered.status, 201);

    let migrated_switch = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        body: Some(json!({"name": "second"})),
    });
    assert_eq!(migrated_switch.status, 200);
    assert_eq!(
        migrated_switch.body["target_root"],
        second_app.display().to_string()
    );
    assert!(second_refine_dir.join("refine.json").exists());

    let newer_app = temp_root.join("newer-app");
    let newer_refine_dir = newer_app.join(".refine");
    fs::create_dir_all(&newer_refine_dir).unwrap();
    fs::write(
        newer_refine_dir.join("refine.json"),
        r#"{"schema_version":999,"refine":{"version":"future"},"created_at":"now","updated_at":"now","settings":{}}"#,
    )
    .unwrap();
    let registered_newer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/register".to_string(),
        body: Some(json!({
            "name": "newer",
            "path": newer_app.display().to_string()
        })),
    });
    assert_eq!(registered_newer.status, 201);
    let blocked_newer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        body: Some(json!({"name": "newer"})),
    });
    assert_eq!(blocked_newer.status, 409);
    assert!(
        blocked_newer.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("newer than this Refine supports")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_resolves_app_scoped_routes_from_active_runtime_app() {
    let temp_root = unique_temp_dir("http-detached-active-app");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let app_registry_root = temp_root.join("run");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = None;
    server.runtime_root = Some(runtime_root.clone());

    let detached_settings = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/settings".to_string(),
        body: None,
    });
    assert_eq!(detached_settings.status, 503);
    assert_eq!(
        detached_settings.body["error"]["code"],
        "target_root_unavailable"
    );

    let attached = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(attached.status, 200);
    assert_eq!(attached.body["target_root"], app_root.display().to_string());
    assert!(app_registry_root.join("apps.json").exists());
    assert!(!runtime_root.join("apps.json").exists());

    let settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({"agent_cli": "smoke-ai"})),
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "smoke-ai");
    assert!(refine_dir.join("settings.json").exists());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"name": "Detached attach gap", "id": "GAP1"})),
    });
    assert_eq!(created.status, 201);
    assert!(refine_dir.join("gaps/GA/P1/gap.json").exists());

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
    let sse_body = String::from_utf8(sse.body).unwrap();
    assert!(sse_body.contains("event: project_updated"));
    assert!(sse_body.contains("\"gap_count\":1"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_governance_guidance_and_reporters() {
    let temp_root = unique_temp_dir("http-project-config");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let governance = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/governance".to_string(),
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
        body: Some(json!({"product": "Refine", "constitution": "Be useful"})),
    });
    assert_eq!(generated.status, 200);
    assert_eq!(generated.body["ok"], true);
    assert!(generated.body["rules"].as_array().unwrap().len() >= 2);

    let guidance = server.handle(ApiRequest {
        method: "PUT".to_string(),
        path: "/api/guidance".to_string(),
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
        body: Some(json!({"name": "Buddy"})),
    });
    assert_eq!(reporter_one.status, 201);
    let reporter_one_id = reporter_one.body["reporter"]["id"].as_u64().unwrap();
    let reporter_two = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/reporters".to_string(),
        body: Some(json!({"name": "Alex"})),
    });
    let reporter_two_id = reporter_two.body["reporter"]["id"].as_u64().unwrap();

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/reporters/{reporter_one_id}"),
        body: Some(json!({"name": "Buddy Williams"})),
    });
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.body["new"], "Buddy Williams");

    let merged = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/reporters/{reporter_one_id}/merge"),
        body: Some(json!({"target_id": reporter_two_id})),
    });
    assert_eq!(merged.status, 200);
    assert_eq!(merged.body["ok"], true);

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/reporters".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["reporters"].as_array().unwrap().len(), 1);
    assert!(refine_dir.join("governance.json").exists());
    assert!(refine_dir.join("guidance.json").exists());
    assert!(refine_dir.join("reporters.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reports_dashboard_diagnostics_target_app_nodes_and_cluster() {
    let temp_root = unique_temp_dir("http-status-surfaces");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        temp_root.join("package.json"),
        r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
    )
    .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "target_app_start_command": "npm run dev",
            "target_app_auto_build": "never"
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP1", "name": "Dashboard Gap"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP2", "name": "Finished Dashboard Gap"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps".to_string(),
        body: Some(json!({"id": "GAP3", "name": "Cancelled Dashboard Gap"})),
    });
    let create_node = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        body: Some(json!({"id": "refine2"})),
    });
    assert_eq!(create_node.status, 200);
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP1/rounds".to_string(),
        body: Some(json!({"reporter": "Alice", "actual": "Needs work", "target": "Works"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP2/rounds".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "assignee": "Carol",
            "actual": "Needs work",
            "target": "Works"
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/GAP3/rounds".to_string(),
        body: Some(json!({"reporter": "Bob", "actual": "Needs work", "target": "Works"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GAP2"],
            "update": {"status": "done"}
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/gaps/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GAP3"],
            "update": {"status": "cancelled"}
        })),
    });
    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-gaps".to_string(),
        body: Some(json!({
            "target_node_id": "refine2",
            "selected_ids": ["GAP3"],
            "filter": {}
        })),
    });
    assert_eq!(transfer.status, 200);
    FileActivityService::new(&refine_dir)
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
    let rebuilt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cache/rebuild".to_string(),
        body: None,
    });
    assert_eq!(rebuilt.status, 200);

    let dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=current".to_string(),
        body: None,
    });
    assert_eq!(dashboard.status, 200);
    assert_eq!(dashboard.body["node_filter"], "current");
    assert_eq!(dashboard.body["counts"]["backlog"], 1);
    assert_eq!(
        dashboard.body["counts"]["cancelled"],
        serde_json::Value::Null
    );
    assert_eq!(dashboard.body["active_node_id"], "default");
    assert_eq!(dashboard.body["activity"][0]["id"], "act-dashboard");
    let all_dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_dashboard.status, 200);
    assert_eq!(all_dashboard.body["node_filter"], "all");
    assert_eq!(all_dashboard.body["counts"]["backlog"], 1);
    assert_eq!(all_dashboard.body["counts"]["cancelled"], 1);
    assert_eq!(
        all_dashboard.body["counts"],
        all_dashboard.body["all_node_counts"]
    );
    let assignee_stats = dashboard.body["assignee_stats"].as_array().unwrap();
    let alice = assignee_stats
        .iter()
        .find(|row| row["assignee"] == "Alice")
        .unwrap();
    assert_eq!(alice["assigned"], 1);
    assert_eq!(alice["active"], 1);
    assert_eq!(alice["done"], 0);
    assert_eq!(alice["completion_rate"], 0.0);
    let carol = assignee_stats
        .iter()
        .find(|row| row["assignee"] == "Carol")
        .unwrap();
    assert_eq!(carol["assigned"], 1);
    assert_eq!(carol["active"], 0);
    assert_eq!(carol["done"], 1);
    assert_eq!(carol["completion_rate"], 100.0);
    let cached = FileProjectStateStore::new(&refine_dir)
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
        body: None,
    });
    assert_eq!(target.status, 200);
    assert_eq!(target.body["app_url"], "http://127.0.0.1:3000");
    assert_eq!(target.body["has_start_command"], true);

    let generated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/generate-instructions".to_string(),
        body: Some(json!({"kind": "all", "provider": "__local__"})),
    });
    assert_eq!(generated.status, 200);
    assert_eq!(
        generated.body["config"]["start_command"],
        "./.refine/manage-app.sh start"
    );
    assert_eq!(
        generated.body["settings"]["target_app_build_command"],
        "./.refine/manage-app.sh build"
    );
    assert_eq!(generated.body["config"]["tcp_check_port"], "3000");
    let wrapper = fs::read_to_string(temp_root.join(".refine/manage-app.sh")).unwrap();
    assert!(wrapper.contains("START_COMMAND='npm run dev'"));
    assert!(wrapper.contains("BUILD_COMMAND='npm run build'"));

    let generated_operation = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/generate-instructions".to_string(),
        body: Some(json!({"kind": "all", "provider": "__local__", "background": true})),
    });
    assert_eq!(
        generated_operation.status, 202,
        "{:?}",
        generated_operation.body
    );
    let generated_operation_id = generated_operation.body["operation"]["id"]
        .as_str()
        .unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let generated_operation =
        wait_for_operation_status(&registry, generated_operation_id, OperationState::Succeeded);
    assert_eq!(
        generated_operation.result["config"]["start_command"],
        "./.refine/manage-app.sh start"
    );
    let settings = FileSettingsService::new(&refine_dir).load().unwrap();
    assert_eq!(
        settings["target_app_start_command"],
        "./.refine/manage-app.sh start"
    );

    let rebuild = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/runner-workers/target-app-builder/build".to_string(),
        body: None,
    });
    assert_eq!(rebuild.status, 200);
    assert_eq!(rebuild.body["queued"], false);

    let nodes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/nodes".to_string(),
        body: None,
    });
    assert_eq!(nodes.status, 200);
    assert_eq!(nodes.body["nodes"][0]["id"], "default");

    let cluster = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/cluster".to_string(),
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
                assignee: Some("Buddy".to_string()),
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
            launch_mode: "cargo".to_string(),
            executable_path: Some("cargo".to_string()),
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
        target_root: None,
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

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_git_app(repo: &Path) {
    fs::create_dir_all(repo.join(".refine")).unwrap();
    git(repo, &["init", "-b", "main"]).unwrap();
    git(repo, &["config", "user.email", "test@example.com"]).unwrap();
    git(repo, &["config", "user.name", "Test User"]).unwrap();
    fs::write(repo.join("app.txt"), "base\n").unwrap();
    git(repo, &["add", "app.txt"]).unwrap();
    git(repo, &["commit", "-m", "initial"]).unwrap();
}

fn seeded_remote_clone(temp_root: &Path) -> (PathBuf, PathBuf) {
    let remote = temp_root.join("remote.git");
    let seed = temp_root.join("seed");
    let app_root = temp_root.join("app");
    fs::create_dir_all(temp_root).unwrap();
    git(
        temp_root,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .unwrap();
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-b", "main"]).unwrap();
    git(&seed, &["config", "user.email", "test@example.com"]).unwrap();
    git(&seed, &["config", "user.name", "Test User"]).unwrap();
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .unwrap();
    fs::write(seed.join("app.txt"), "initial\n").unwrap();
    git(&seed, &["add", "app.txt"]).unwrap();
    git(&seed, &["commit", "-m", "initial"]).unwrap();
    git(&seed, &["push", "-u", "origin", "main"]).unwrap();
    git(
        temp_root,
        &[
            "clone",
            remote.to_str().unwrap(),
            app_root.to_str().unwrap(),
        ],
    )
    .unwrap();
    (seed, app_root)
}

fn wait_for_http_request_metrics(
    runtime_root: &Path,
) -> Vec<crate::tools::observability::metrics::PerformanceEvent> {
    for _ in 0..20 {
        let report = FileMetricsService::new(runtime_root)
            .report(PerformanceQuery {
                operation: Some("http.request".to_string()),
                ..PerformanceQuery::default()
            })
            .unwrap();
        if !report.events.is_empty() {
            return report.events;
        }
        thread::sleep(Duration::from_millis(25));
    }
    Vec::new()
}

fn wait_for_operation_status(
    registry: &FileOperationRegistry,
    operation_id: &str,
    expected: OperationState,
) -> OperationHandle {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(operation) = registry.status(operation_id)
            && operation.state == expected
        {
            return operation;
        }
        if Instant::now() >= deadline {
            let latest = registry.status(operation_id).ok();
            panic!(
                "timed out waiting for operation {operation_id} to reach {expected:?}: {latest:?}"
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_chat_read_line(
    server: &InProcessWebServer,
    session_id: &str,
    needle: &str,
) -> ApiResponse {
    let mut lines = Vec::new();
    let mut progress_lines = Vec::new();
    for _ in 0..100 {
        let mut read = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/chat/{session_id}/read"),
            body: None,
        });
        if let Some(values) = read.body.get("lines").and_then(|value| value.as_array()) {
            lines.extend(values.iter().cloned());
        }
        if let Some(values) = read
            .body
            .get("progress_lines")
            .and_then(|value| value.as_array())
        {
            progress_lines.extend(values.iter().cloned());
        }
        let has_line = lines
            .iter()
            .any(|line| line.as_str().unwrap_or("").contains(needle));
        if has_line {
            read.body["lines"] = serde_json::Value::Array(lines);
            read.body["progress_lines"] = serde_json::Value::Array(progress_lines);
            return read;
        }
        thread::sleep(Duration::from_millis(25));
    }
    server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/chat/{session_id}/read"),
        body: None,
    })
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

fn write_fake_provider(refine_dir: &Path, name: &str, exit_code: i32, output: &str) {
    let bin_dir = refine_dir.join("provider-bin");
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
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
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
