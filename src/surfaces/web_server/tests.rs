use crate::model::log::LogEntry;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::tools::product::chat::{ChatAttachment, ChatService, FileChatService};
use crate::workflow::{WorkflowAutomation, WorkflowEngine};
use chrono::Utc;
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
use crate::model::goal::{GoalIndexProjection, GoalPriority};
use crate::model::log::ActivityEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner, ProcessSupervisor,
};
use crate::surfaces::web_server::support::{
    runtime_process_status_value, runtime_process_summary_value,
};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::project_state::{
    ActivityProjectionQuery, DashboardProjection, FeatureSummaryProjection, FileProjectStateStore,
    GoalSummaryProjection, PROJECTION_SNAPSHOT_FILE, PROJECTION_SNAPSHOT_VERSION, PageRequest,
    ProjectStateStore, ProjectionQuery, ProjectionSnapshot, RuntimeProjection,
};
use crate::tools::product::work_items::FileWorkItemService;

#[test]
fn web_server_serves_mcp_surface_through_daemon() {
    let server = server_with_projection();

    // The MCP surface is mounted by the always-on daemon web server, so a
    // JSON-RPC tools/call reaches a real capability route without any extra
    // process or transport.
    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/mcp".to_string(),
        body: Some(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "refine_list_goals", "arguments": {}},
        })),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["result"]["isError"], false);
    let goals = &response.body["result"]["structuredContent"]["goals"];
    assert!(goals.as_array().is_some());

    // GET reports server identity so clients can discover the surface.
    let identity = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/mcp".to_string(),
        body: None,
    });
    assert_eq!(identity.status, 200);
    assert_eq!(identity.body["serverInfo"]["name"], "refine");
}

#[test]
fn web_server_routes_work_goal_queries_through_projection() {
    let mut server = server_with_projection();
    server.projection.goals.insert(
        "GOAL2".to_string(),
        GoalSummaryProjection {
            goal: GoalIndexProjection {
                id: "GOAL2".to_string(),
                name: "Settings route".to_string(),
                status: GoalStatus::Done,
                priority: GoalPriority::High,
                reporter: Some("Alice".to_string()),
                assignee: Some("Alice".to_string()),
                round_count: 3,
                created: "created2".to_string(),
                updated: "updated2".to_string(),
                branch_name: None,
                node_id: Some("node-b".to_string()),
                feature_id: Some("FEA1".to_string()),
                feature_order: Some(1),
                json_path: "goals/02/GOAL2/goal.json".to_string(),
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
            status: GoalStatus::Done,
            goal_ids: vec!["GOAL2".to_string()],
            rollup: FeatureRollup {
                status: GoalStatus::Done,
                goal_count: 1,
                done_count: 1,
                active_count: 0,
                failed_count: 0,
                cancelled_count: 0,
                blocked_count: 0,
                next_goal: None,
            },
        },
    );
    let response = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals".to_string(),
        body: None,
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["goals"].as_array().unwrap().len(), 2);
    assert_eq!(response.body["counts"]["todo"], 1);
    assert_eq!(response.body["counts"]["done"], 1);

    let filtered = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?reporter=Alice&feature=FEA1&rounds_gte=2&sort=priority&dir=desc&limit=1"
            .to_string(),
        body: None,
    });
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.body["goals"][0]["id"], "GOAL2");
    assert_eq!(filtered.body["filtered_counts"]["done"], 1);
    assert_eq!(filtered.body["matching_ids"], json!(["GOAL2"]));
    assert_eq!(filtered.body["page"]["total"], 1);
    assert!(filtered.body.get("facets").is_none());

    let status_facets = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?status=todo&reporter=Alice&feature=FEA1&rounds_gte=2&facets=1"
            .to_string(),
        body: None,
    });
    assert_eq!(status_facets.status, 200);
    assert_eq!(status_facets.body["goals"].as_array().unwrap().len(), 0);
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
        .goals
        .get_mut("GOAL1")
        .unwrap()
        .goal
        .status = GoalStatus::Failed;
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
            && item["message"] == "1 failed Goal(s) need recovery"
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

#[test]
fn static_runtime_settings_expose_state_sync_controls() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let runtime = fs::read_to_string(static_root.join("js/features/settings_runtime.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    assert!(runtime.contains(r#"data-testid="runtime-state-sync-now""#));
    assert!(runtime.contains(r#"data-testid="runtime-state-sync-debounce""#));
    assert!(runtime.contains(r#"data-testid="runtime-project-update-pulse""#));
    assert!(runtime.contains(r#"api("POST", "/api/project/sync", {})"#));
    assert!(runtime.contains("resolveBackgroundOperationResponse"));
    assert!(
        runtime.contains(r##"state_sync_debounce_seconds: $("#s-state-sync-debounce").value"##)
    );
    assert!(runtime.contains(
        r##"project_update_pulse_interval_seconds: $("#s-project-update-pulse").value"##
    ));
    assert!(!runtime.contains(r#"data-testid="source-promotion-section""#));
    assert!(releases.contains(r#"data-testid="source-promotion-section""#));
    assert!(releases.contains(r#"data-testid="source-promotion-check""#));
    assert!(releases.contains(r#"data-testid="source-promotion-promote""#));
    assert!(releases.contains("/api/system/source/check"));
    assert!(releases.contains("/api/system/source/promote"));
    assert!(releases.contains("Refine is restarting; reconnecting"));
}

#[test]
fn static_main_nav_exposes_refine_source_update_affordance() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();
    let init = fs::read_to_string(static_root.join("js/init.js")).unwrap();

    assert!(index.contains(r#"data-testid="nav-source-update""#));
    assert!(index.contains("hidden disabled"));
    assert!(releases.contains("const sourceUpdate = result.source_update || {}"));
    assert!(releases.contains("button.disabled = sourceUpdate.enabled !== true"));
    assert!(releases.contains(r#"fetchRemote ? "/api/system/source/check""#));
    assert!(releases.contains("const confirmed = window.confirm("));
    assert!(releases.contains(r#"api("POST", "/api/system/source/promote", {})"#));
    assert!(init.contains("initSourceUpdateNav()"));
}

#[test]
fn source_update_status_integration_drives_browser_states_across_reconnect() {
    use crate::tools::host::source_promotion::{
        SOURCE_PROMOTION_STATE_FILE, SourcePromotionOperation,
    };

    let temp_root = unique_temp_dir("source-update-status-integration");
    let runtime_root = temp_root.join("run/8080");
    let (seed, target_root) = seeded_remote_clone(&temp_root);
    fs::create_dir_all(target_root.join("src")).unwrap();
    fs::create_dir_all(target_root.join("scripts")).unwrap();
    fs::write(
        target_root.join("Cargo.toml"),
        "[package]\nname = \"refine\"\n",
    )
    .unwrap();
    fs::write(target_root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(target_root.join("scripts/install.sh"), "#!/bin/sh\n").unwrap();
    fs::write(target_root.join("r"), "#!/bin/sh\n").unwrap();
    git(&target_root, &["add", "."]).unwrap();
    git(
        &target_root,
        &["commit", "-m", "add Refine source entrypoints"],
    )
    .unwrap();
    git(&target_root, &["push", "origin", "main"]).unwrap();
    git(&seed, &["pull", "--ff-only"]).unwrap();
    fs::write(seed.join("remote.txt"), "new source\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "new source commit"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor.set_agents_paused(true).unwrap();
    supervisor.set_background_processes_stopped(true).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(target_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let available = server.handle_source_status_for_checkout(true, target_root.clone());
    assert_eq!(available.status, 200);
    assert_eq!(available.body["target_app_is_refine"], true);
    assert_eq!(available.body["source_update"]["visible"], true);
    assert_eq!(available.body["source_update"]["enabled"], true);
    assert_eq!(available.body["source_update"]["state"], "available");
    let current_commit = available.body["source"]["current_commit"]
        .as_str()
        .unwrap()
        .to_string();
    let available_commit = available.body["source"]["available_commit"]
        .as_str()
        .unwrap()
        .to_string();

    fs::write(target_root.join("dirty.txt"), "leave untouched\n").unwrap();
    let blocked = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(blocked.status, 200);
    assert_eq!(blocked.body["source_update"]["visible"], true);
    assert_eq!(blocked.body["source_update"]["enabled"], false);
    assert_eq!(blocked.body["source_update"]["state"], "blocked");
    fs::remove_file(target_root.join("dirty.txt")).unwrap();

    let mut operation = SourcePromotionOperation {
        id: "source-test".to_string(),
        status: "running".to_string(),
        stage: "restart_daemon".to_string(),
        message: "Source activated; restarting Refine".to_string(),
        checkout_path: target_root.display().to_string(),
        from_commit: current_commit,
        to_commit: available_commit,
        started_at: "2026-07-21T00:00:00Z".to_string(),
        updated_at: "2026-07-21T00:00:01Z".to_string(),
        error: None,
        rollback_attempted: false,
        rollback_succeeded: None,
        recovery: None,
    };
    fs::write(
        runtime_root.join(SOURCE_PROMOTION_STATE_FILE),
        serde_json::to_vec_pretty(&operation).unwrap(),
    )
    .unwrap();
    let reconnecting = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(reconnecting.status, 200);
    assert_eq!(reconnecting.body["source_update"]["enabled"], false);
    assert_eq!(reconnecting.body["source_update"]["state"], "updating");
    assert_eq!(
        reconnecting.body["source_update"]["title"],
        "Source activated; restarting Refine"
    );

    operation.status = "failed".to_string();
    operation.stage = "restart_daemon".to_string();
    operation.message = "Source promotion failed during restart_daemon".to_string();
    operation.error = Some("restart failed".to_string());
    operation.recovery = Some("Refine was restored; inspect and retry".to_string());
    fs::write(
        runtime_root.join(SOURCE_PROMOTION_STATE_FILE),
        serde_json::to_vec_pretty(&operation).unwrap(),
    )
    .unwrap();
    let failed = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(failed.status, 200);
    assert_eq!(failed.body["source"]["operation"]["status"], "failed");
    assert_eq!(failed.body["source_update"]["enabled"], true);
    assert_eq!(failed.body["source_update"]["state"], "available");

    server.target_root = Some(temp_root.join("not-refine"));
    let hidden = server.handle_source_status_for_checkout(false, target_root.clone());
    assert_eq!(hidden.status, 200);
    assert_eq!(hidden.body["target_app_is_refine"], false);
    assert_eq!(hidden.body["source_update"]["visible"], false);
    assert_eq!(hidden.body["source_update"]["enabled"], false);
    assert_eq!(hidden.body["source_update"]["state"], "hidden");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn source_promotion_api_preserves_unconfirmed_request_compatibility() {
    let server = server_with_projection();
    for body in [
        None,
        Some(json!({})),
        Some(json!({"confirmed": false})),
        Some(json!({"confirmed": true})),
    ] {
        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/system/source/promote".to_string(),
            body,
        });
        assert_eq!(response.status, 503);
        assert_eq!(response.body["error"]["code"], "runtime_root_unavailable");
    }
}

#[test]
fn static_project_settings_explain_governance_and_quality_effects() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let governance =
        fs::read_to_string(static_root.join("js/features/settings_governance.js")).unwrap();
    let quality = fs::read_to_string(static_root.join("js/features/settings_quality.js")).unwrap();

    assert!(governance.contains(r#"data-testid="governance-explanation""#));
    assert!(governance.contains("A rule violation stops the Goal before"));
    assert!(governance.contains("do not start a check now"));

    assert!(quality.contains(r#"data-testid="quality-explanation""#));
    assert!(quality.contains("Passing checks advance the Goal to review"));
    assert!(quality.contains("preserve the candidate"));
    assert!(quality.contains("do not start a run now"));
}

#[test]
fn static_releases_surface_separates_prepare_from_confirmed_publish() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let settings = fs::read_to_string(static_root.join("js/features/settings.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    assert!(index.contains("settings_releases.js"));
    let node_tabs = settings
        .split("  node: {")
        .nth(1)
        .and_then(|node| node.split("  project: {").next())
        .expect("Node settings surface");
    assert!(node_tabs.contains(r#"{ slug: "runtime", label: "Runtime Config" }"#));
    assert!(node_tabs.contains(r#"{ slug: "releases", label: "Refine (dev)" }"#));
    assert!(
        node_tabs.find(r#"slug: "runtime""#).unwrap()
            < node_tabs.find(r#"slug: "releases""#).unwrap()
    );
    assert!(
        node_tabs
            .trim_end()
            .ends_with("{ slug: \"releases\", label: \"Refine (dev)\" },\n    ],\n  },")
    );
    assert!(releases.contains(r#"data-testid="release-bump""#));
    assert!(releases.contains(r#"data-testid="release-preview""#));
    assert!(releases.contains(r#"data-testid="release-prepare""#));
    assert!(releases.contains(r#"data-testid="release-publish""#));
    assert!(releases.contains("explicit confirmation"));
    assert!(releases.contains("/api/system/releases/prepare"));
    assert!(releases.contains("/api/system/releases/publish"));
    assert!(releases.contains("/retry"));
}

#[test]
fn release_api_previews_semver_and_rejects_unconfirmed_publication() {
    let runtime_root = unique_temp_dir("http-releases");
    fs::create_dir_all(&runtime_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let plan = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/releases/plan".to_string(),
        body: Some(json!({"bump": "patch"})),
    });
    assert_eq!(plan.status, 200, "{}", plan.body);
    assert_eq!(plan.body["plan"]["current_version"], "4.0.0");
    assert_eq!(plan.body["plan"]["proposed_version"], "4.0.1");
    assert_eq!(plan.body["plan"]["proposed_tag"], "4.0.1");

    let publish = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/releases/publish".to_string(),
        body: Some(json!({
            "confirmed": false,
            "preparation_id": "browser-controlled-value"
        })),
    });
    assert_eq!(publish.status, 400);
    assert!(
        publish.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("confirmed=true")
    );
    assert!(!releases_request_body_accepts_candidate_objects());

    let tampered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/releases/publish".to_string(),
        body: Some(json!({
            "confirmed": true,
            "preparation_id": "browser-controlled-value",
            "candidate": {
                "commit": "attacker-selected-commit",
                "worktree": "/tmp/attacker-selected-worktree"
            }
        })),
    });
    assert_eq!(tampered.status, 404, "{}", tampered.body);
    assert!(
        tampered.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("release request browser-controlled-value was not found")
    );

    fs::remove_dir_all(runtime_root).unwrap();
}

fn releases_request_body_accepts_candidate_objects() -> bool {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/surfaces/web/static/js/features/settings_releases.js"),
    )
    .unwrap();
    source.contains("{ candidate, confirmed: true }")
}

#[test]
fn static_import_modal_exposes_feature_import_surface() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();
    let import_modes = fs::read_to_string(static_root.join("js/features/goals-import.js")).unwrap();
    let import_modal =
        fs::read_to_string(static_root.join("js/features/goals-import-modal.js")).unwrap();
    let import_prepare =
        fs::read_to_string(static_root.join("js/features/goals-import-prepare.js")).unwrap();

    assert!(index.contains(r#"data-testid="nav-import-goals">Import</a>"#));
    assert!(commands.contains(r#"title: "Import""#));
    assert!(import_modes.contains(r#"mode: "feature""#));
    for label in [
        "Import Feature",
        "Import Goals",
        "Import Goals (.csv)",
        "Upload Goals (.csv)",
    ] {
        assert!(import_modes.contains(label), "missing import label {label}");
    }
    assert!(import_modal.contains(r#"data-testid="import-feature-text""#));
    assert!(import_modes.contains("Extract Feature"));
    assert!(import_modal.contains("startImportExtractOperation(text,"));
    assert!(import_modal.contains("force_provider: true"));
    assert!(import_modal.contains("queueImportPreparation(started.operation, activeMode"));
    assert!(import_modal.contains("startImportCsvParseOperation(csvText"));
    assert!(import_prepare.contains("function planDraftPayloadFromResult"));
    assert!(import_prepare.contains("async function startImportExtractOperation"));
    assert!(import_prepare.contains("async function startImportCsvParseOperation"));
    assert!(import_prepare.contains("async function saveImportDraftReviewState"));
    assert!(import_prepare.contains("async function reviewPlanFeatureDraftPayload"));
}

#[test]
fn static_plan_chat_shows_initial_design_prompt() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();

    assert!(toolbar.contains("function renderChatOutput"));
    assert!(toolbar.contains("What do you want to design together?"));
    assert!(toolbar.contains("renderChatOutput(active)"));
    assert!(toolbar.contains("renderChatOutput(tab)"));
    assert!(toolbar.contains(r#"data-testid="plan-draft-goal""#));
    assert!(toolbar.contains("async function draftGoalFromPlan"));
    assert!(toolbar.contains(r#"purpose: "plan_goal""#));
    assert!(toolbar.contains(r#"testIdPrefix: "plan-goal-draft""#));
    assert!(commands.contains(r#"id: "plan.draft_goal""#));
    assert!(commands.contains(r#"title: "Draft Goal from plan""#));
}

#[test]
fn static_goal_log_tail_uses_toolbar_and_shared_sse_activity() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let goal_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let toolbar_css = fs::read_to_string(static_root.join("css/toolbar.css")).unwrap();

    assert!(goal_detail.contains(r#"data-testid="goal-action-watch-logs""#));
    assert!(goal_detail.contains("openGoalLogTail({ goalId: goal.id"));
    assert!(toolbar.contains("function openGoalLogTail"));
    assert!(toolbar.contains("function loadGoalLogTail"));
    assert!(toolbar.contains("/api/activity?${params}"));
    assert!(toolbar.contains(r#"dir: "desc""#));
    assert!(toolbar.contains(r#"role="log" aria-live="polite""#));
    assert!(toolbar.contains("function handleGoalLogSseEvent"));
    assert!(common.contains(r#"addEventListener("goal_log_added""#));
    assert!(toolbar_css.contains(".goal-log-tail"));
}

#[test]
fn static_toolbar_keeps_supervisor_agent_visible_and_uses_shared_chat() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let toolbar_css = fs::read_to_string(static_root.join("css/toolbar.css")).unwrap();

    assert!(toolbar.contains(r#"label: "Supervisor", mode: "supervisor""#));
    assert!(toolbar.contains("STANDARD_TOOLBAR_TAB_ORDER = [SUPERVISOR_TAB_ID"));
    assert!(toolbar.contains(r#"api("GET", "/api/supervisor-agent")"#));
    assert!(toolbar.contains(r#"api("POST", "/api/supervisor-agent/session"#));
    assert!(toolbar.contains("renderChatPanel(active, chatOptions)"));
    assert!(toolbar.contains("recordSupervisorAgentEvents(snapshot?.events)"));
    assert!(toolbar.contains(r#"category: "supervisor""#));
    assert!(!toolbar.contains(r#"data-testid="supervisor-agent-events""#));
    assert!(toolbar.contains(r#"data-testid="supervisor-agent-conversation""#));
    assert!(toolbar_css.contains(".supervisor-agent-summary"));
    assert!(!toolbar_css.contains(".supervisor-agent-events"));
}

#[test]
fn static_system_log_exposes_sources_and_diagnostic_details() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let toolbar_css = fs::read_to_string(static_root.join("css/toolbar.css")).unwrap();

    assert!(common.contains("if (details) payload.details = details"));
    assert!(common.contains(r#"details: { operation_id: response.operation.id }"#));
    assert!(common.contains("function activitySystemOperationDetails"));
    assert!(common.contains("details.activity_id = entry.id"));
    assert!(common.contains("details.goal_id = entry.goal_id"));
    assert!(common.contains("details: activitySystemOperationDetails(entry)"));
    assert!(commands.contains(r#"details: { operation_id: operationId }"#));
    assert!(commands.contains(r#"details: { operation_id: response.operation.id }"#));
    assert!(toolbar.contains("details: payload?.details ?? null"));
    assert!(toolbar.contains("function systemOperationDetailEntries"));
    assert!(toolbar.contains(r#"data-testid="system-log-status""#));
    assert!(toolbar.contains(r#"data-testid="system-log-category""#));
    assert!(toolbar.contains(r#"data-testid="system-log-details""#));
    assert!(toolbar.contains(r#"data-testid="system-log-detail""#));
    assert!(toolbar.contains("existing.category !== item.category"));
    assert!(toolbar.contains("formatSystemOperationDetails(existing.details) !== itemDetails"));
    assert!(toolbar_css.contains(".system-log-status"));
    assert!(toolbar_css.contains(".system-log-category"));
    assert!(toolbar_css.contains(".system-log-detail dd"));
}

#[test]
fn static_work_item_tables_use_shared_readable_name_layout() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common_css = fs::read_to_string(static_root.join("css/common.css")).unwrap();
    let goals_css = fs::read_to_string(static_root.join("css/goals.css")).unwrap();
    let goals_list = fs::read_to_string(static_root.join("js/features/goals-list.js")).unwrap();
    let features = fs::read_to_string(static_root.join("js/features/features.js")).unwrap();

    assert!(common_css.contains(".work-items-table"));
    assert!(common_css.contains(".work-item-name-col"));
    assert!(common_css.contains(".work-item-name-cell"));
    assert!(!common_css.contains(".table-scroll {\n  max-width: 100%;\n  overflow-x: auto;"));
    assert!(!common_css.contains("min-width: var(--work-items-table-min-width"));
    assert!(common_css.contains("overflow-wrap: break-word"));
    assert!(common_css.contains("word-break: normal"));
    assert!(common_css.contains("width: var(--work-item-select-width, 4%)"));

    assert_eq!(goals_css.matches("--work-item-name-width: 20%").count(), 2);
    assert!(goals_css.contains("--work-item-select-width: 4%"));
    assert!(goals_css.contains(".features-col-next {\n  width: 17%;"));
    assert!(goals_css.contains(".features-col-updated {\n  width: 9%;"));
    assert!(!goals_css.contains(".features-name-cell {\n  overflow-wrap: anywhere;"));

    for source in [goals_list.as_str(), features.as_str()] {
        assert!(source.contains("work-items-table"));
        assert!(source.contains("work-item-name-col"));
        assert!(source.contains("work-item-name-cell"));
    }
}

#[test]
fn static_goal_detail_logs_feature_blocking_notice_to_system() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let goals_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();

    assert!(goals_detail.contains("feature_blocking_notice"));
    assert!(goals_detail.contains(r#"data-testid="goal-feature-blocking-banner""#));
    assert!(goals_detail.contains("function recordFeatureBlockingNotice"));
    assert!(goals_detail.contains("recordUiNotice(notice.message"));
    assert!(goals_detail.contains(r#"source: "workflow""#));
}

#[test]
fn static_goal_detail_uses_shared_governance_review_state_helpers() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let goals_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();

    assert!(common.contains("function governanceReviewStatus"));
    assert!(common.contains(r#""pass", "passed""#));
    assert!(common.contains("function reviewStateClass"));
    assert!(goals_detail.contains("governanceReviewStatus(round)"));
    assert!(goals_detail.contains("governanceReviewStatus(latest)"));
    assert!(goals_detail.contains("reviewStateClass(states.product)"));
    assert!(goals_detail.contains("reviewStateClass(states.constitution)"));
    assert!(!goals_detail.contains(r#"product_state === "pass""#));
    assert!(!goals_detail.contains(r#"constitution_state === "pass""#));
}

#[test]
fn static_goal_reports_and_bulk_jira_export_use_the_correct_surfaces() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let goals_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();
    let goals_list = fs::read_to_string(static_root.join("js/features/goals-list.js")).unwrap();
    let goals_bulk = fs::read_to_string(static_root.join("js/features/goals-bulk.js")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();

    assert!(goals_detail.contains("rnd.implementation_report"));
    assert!(goals_detail.contains(r#"data-testid="goal-implementation-report""#));
    assert!(goals_detail.contains(r#"data-testid="goal-implementation-report-body""#));
    assert!(goals_detail.contains("rnd.implementation_reported_at"));
    assert!(!goals_detail.contains(r#"data-testid="goal-action-export-jira""#));
    assert!(!goals_detail.contains("/export/jira"));
    assert!(goals_list.contains(r#"data-testid="goals-bulk-export-jira""#));
    assert!(goals_list.contains(r##"bindCommand("#bulk-export-jira", "goals.bulk.export_jira")"##));
    assert!(goals_bulk.contains("function exportSelectedGoalsForJira"));
    assert!(goals_bulk.contains(r#"api("POST", "/api/goals/export/jira""#));
    assert!(goals_bulk.contains("..._selectionRequestFields()"));
    assert!(goals_bulk.contains("waitForGoalsJiraExportOperation"));
    assert!(goals_bulk.contains("GOALS_JIRA_EXPORT_OPERATION_KEY"));
    assert!(goals_bulk.contains("/retry`"));
    assert!(goals_list.contains("syncGoalsJiraExportOperation()"));
    assert!(common.contains(r#"err.code = "operation_interrupted""#));
    assert!(commands.contains(r#"id: "goals.bulk.export_jira""#));
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
        path: "/work/goals".to_string(),
        headers: BTreeMap::from([("origin".to_string(), "https://example.com".to_string())]),
        body: Some(br#"{"name":"Bad"}"#.to_vec()),
    });
    assert_eq!(forbidden.status, 403);

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

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_creates_goal_from_new_goal_modal_payload() {
    let temp_root = unique_temp_dir("http-goal-create-modal");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "assignee": "Bob",
            "prompt": "Pressing pause should freeze the board and show a paused state.",
            "priority": "high"
        })),
    });

    assert_eq!(created.status, 201);
    let goal_id = created.body["goal"]["id"].as_str().unwrap();
    assert_eq!(
        created.body["goal"]["name"],
        "Pressing pause should freeze the board and show a paused state."
    );
    assert_eq!(created.body["goal"]["priority"], "high");
    assert_eq!(created.body["goal"]["reporter"], "Alice");
    assert_eq!(created.body["goal"]["assignee"], "Bob");
    assert_eq!(created.body["goal"]["round_count"], 1);

    let detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/goals/{goal_id}"),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(
        detail.body["goal"]["rounds"][0]["prompt"],
        "Pressing pause should freeze the board and show a paused state."
    );
    assert_eq!(detail.body["goal"]["rounds"][0]["assignee"], "Bob");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_instantly_promotes_new_goal_when_configured() {
    let temp_root = unique_temp_dir("http-goal-create-instant-promote");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    FileSettingsService::with_active_root(&refine_dir, &runtime_root)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "id": "GOAL1",
            "name": "Instantly promoted Goal"
        })),
    });

    assert_eq!(created.status, 201);
    assert_eq!(created.body["goal"]["status"], "todo");
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_handles_new_goal_duplicate_decisions() {
    let temp_root = unique_temp_dir("http-goal-duplicate-modal");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let body = json!({
        "reporter": "Alice",
        "prompt": "Duplicate target state",
        "priority": "low"
    });
    let original = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(body.clone()),
    });
    assert_eq!(original.status, 201);
    let original_id = original.body["goal"]["id"].as_str().unwrap().to_string();

    let duplicate = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(body.clone()),
    });
    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.body["error"]["code"], "duplicate_goal");
    assert_eq!(
        duplicate.body["error"]["duplicate"]["match"]["id"],
        original_id
    );

    let ignored = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "prompt": "Duplicate target state",
            "duplicate_decision": "duplicate"
        })),
    });
    assert_eq!(ignored.status, 200);
    assert_eq!(ignored.body["created"], false);
    assert_eq!(ignored.body["duplicate_action"], "duplicate");

    FileWorkItemService::new(&refine_dir)
        .transition_goal_status(&original_id, GoalStatus::Todo)
        .unwrap();
    let moved = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "prompt": "Duplicate target state",
            "duplicate_decision": "move_original_to_backlog"
        })),
    });
    assert_eq!(moved.status, 200);
    assert_eq!(moved.body["created"], false);
    assert_eq!(moved.body["duplicate_action"], "move_original_to_backlog");
    assert_eq!(
        moved.body["move"],
        json!({"moved": true, "from": "todo", "to": "backlog"})
    );

    let imported = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "prompt": "Duplicate target state",
            "duplicate_decision": "original"
        })),
    });
    assert_eq!(imported.status, 201);
    let imported_id = imported.body["goal"]["id"].as_str().unwrap();
    assert_ne!(imported_id, original_id);

    let list = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?q=Duplicate%20target%20state".to_string(),
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
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
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
    fs::remove_dir_all(temp_root).unwrap();
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
fn local_http_daemon_serves_website_and_markdown_from_repo_root() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: Some(repo_root),
    };

    let index = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(index.status, 200);
    assert_eq!(index.content_type, "text/html; charset=utf-8");
    assert!(
        String::from_utf8(index.body)
            .unwrap()
            .contains("Agentic Software Delivery")
    );

    let docs_home = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/docs".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(docs_home.status, 200);
    assert_eq!(docs_home.content_type, "text/html; charset=utf-8");
    let docs_home = String::from_utf8(docs_home.body).unwrap();
    assert!(docs_home.contains("<h1 id=\"docs-home-title\">How Refine works.</h1>"));
    assert!(docs_home.contains("Browser Details"));
    assert!(docs_home.contains(r#"href="/read/docs/intent/04-surfaces/05-agent.md""#));

    let raw_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/docs/runbooks/install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(raw_doc.status, 200);
    assert_eq!(raw_doc.content_type, "text/markdown; charset=utf-8");
    assert!(
        String::from_utf8(raw_doc.body)
            .unwrap()
            .contains("# Install Refine")
    );

    let compatibility_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/docs/agent-install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(compatibility_doc.status, 200);
    assert!(
        String::from_utf8(compatibility_doc.body)
            .unwrap()
            .contains("docs/runbooks/install.md")
    );

    let rendered_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/docs/runbooks/install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(rendered_doc.status, 200);
    assert_eq!(rendered_doc.content_type, "text/html; charset=utf-8");
    let rendered_doc = String::from_utf8(rendered_doc.body).unwrap();
    assert!(rendered_doc.contains("<h1>Install Refine</h1>"));
    assert!(rendered_doc.contains("Raw Markdown"));
    assert!(
        rendered_doc.contains(r#"<div class="menu-docs" aria-label="Documentation sections">"#)
    );
    assert!(!rendered_doc.contains(r#"class="reader-nav""#));
    assert_eq!(rendered_doc.matches(r#"class="doc-pager""#).count(), 2);
    assert!(rendered_doc.contains(r#">Docs home</a>"#));
    assert!(rendered_doc.contains(r#"href="/docs""#));
    assert!(rendered_doc.contains("/read/docs/intent/02-foundation/01-node.md"));

    let design_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/docs/intent/01-design.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(design_doc.status, 200);
    let design_doc = String::from_utf8(design_doc.body).unwrap();
    assert_eq!(design_doc.matches(r#"class="doc-pager""#).count(), 2);
    assert!(design_doc.contains(
        r#"<a class="doc-pager-link" href="/read/docs/intent/README.md"><span>Previous</span><strong>Design Intent</strong></a>"#
    ));
    assert!(
        design_doc.contains(r#"<a class="doc-pager-link" href="/read/docs/intent/02-foundation/01-node.md"><span>Next</span><strong>Node</strong></a>"#)
    );

    let intent_toc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/docs/intent/README.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(intent_toc.status, 200);
    let intent_toc = String::from_utf8(intent_toc.body).unwrap();
    assert!(intent_toc.contains("<h1>Design Intent</h1>"));
    assert!(!intent_toc.contains("<h1>Table of Contents</h1>"));
    assert!(intent_toc.contains(r#"href="/read/docs/intent/01-design.md""#));
    assert!(
        intent_toc
            .contains(r#"href="/read/docs/intent/03-capabilities/03-workflow/00-overview.md""#)
    );

    let hidden = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/Cargo.toml".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_ne!(hidden.status, 200);
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

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_transitions_goal_and_refine_dir() {
    let temp_root = unique_temp_dir("http-transition");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
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
        path: "/work/goals/GOAL1/transition".to_string(),
        body: Some(json!({"status": "todo"})),
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.body["goal"]["status"], "todo");
    assert!(
        fs::read_to_string(goal_dir.join("goal.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    let patch_response = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: Some(json!({"status": "backlog"})),
    });
    assert_eq!(patch_response.status, 200);
    assert_eq!(patch_response.body["goal"]["status"], "backlog");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_creates_and_shows_goal() {
    let temp_root = unique_temp_dir("http-create-show");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let create = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Created by API"})),
    });
    assert_eq!(create.status, 201);
    assert_eq!(create.body["goal"]["id"], "GOAL1");
    assert!(refine_dir.join("goals/GO/AL1/goal.json").exists());

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["goal"]["name"], "Created by API");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_exports_selected_goals_for_jira() {
    let temp_root = unique_temp_dir("http-goals-jira-export");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GOAL1", "Export first delivery"),
        ("GOAL2", "Export second delivery"),
        ("GOAL3", "Leave this Goal out"),
    ] {
        service.create_goal_summary(name, Some(id)).unwrap();
        service
            .append_goal_round_summary(id, "Auditor", &format!("Implement {id}"))
            .unwrap();
        service
            .update_latest_goal_round_implementation_report(
                id,
                &format!("Added Jira evidence for {id}."),
            )
            .unwrap();
    }

    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/export/jira".to_string(),
        body: Some(json!({"selected_ids": ["GOAL2", "GOAL1"]})),
    });

    assert_eq!(response.status, 202);
    assert_eq!(response.body["operation"]["owner"], "goals:jira-export");
    assert_eq!(response.body["operation"]["status"], "running");
    let operation_id = response.body["operation"]["id"].as_str().unwrap();
    let operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        operation_id,
        OperationState::Succeeded,
    );
    assert_eq!(operation.progress["stage"], "complete");
    assert_eq!(operation.progress["completed"], 2);
    assert_eq!(operation.result["export"]["format"], "jira_csv");
    assert_eq!(
        operation.result["export"]["filename"],
        "refine-goals-jira.csv"
    );
    assert_eq!(operation.result["export"]["goal_count"], 2);
    assert_eq!(
        operation.result["export"]["goal_ids"],
        json!(["GOAL1", "GOAL2"])
    );
    let csv = operation.result["export"]["csv"].as_str().unwrap();
    assert!(csv.starts_with("Summary,Description,Work Type,Priority"));
    assert!(csv.contains("Added Jira evidence for GOAL1."));
    assert!(csv.contains("Added Jira evidence for GOAL2."));
    assert!(!csv.contains("Leave this Goal out"));

    let empty = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/export/jira".to_string(),
        body: Some(json!({"selected_ids": []})),
    });
    assert_eq!(empty.status, 202);
    let empty_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        empty.body["operation"]["id"].as_str().unwrap(),
        OperationState::Failed,
    );
    assert_eq!(
        empty_operation.error.unwrap()["message"],
        "Select at least one Goal to export for Jira"
    );

    let (logs, _, _) = FileOperationRegistry::new(&runtime_root)
        .page_logs(operation_id, 20, 0)
        .unwrap();
    assert!(
        logs.iter()
            .any(|entry| entry.message == "Jira CSV export completed")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_cancels_and_recovers_durable_jira_exports() {
    let temp_root = unique_temp_dir("http-goals-jira-export-lifecycle");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Recoverable export", Some("GOAL1"))
        .unwrap();

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/export/jira".to_string(),
        body: Some(json!({"selected_ids": ["GOAL1"]})),
    });
    assert_eq!(started.status, 202);
    let operation_id = started.body["operation"]["id"].as_str().unwrap();
    let cancelled = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{operation_id}/cancel"),
        body: None,
    });
    assert_eq!(cancelled.status, 200);
    assert_eq!(cancelled.body["operation"]["status"], "cancelled");
    thread::sleep(Duration::from_millis(25));
    assert_eq!(
        FileOperationRegistry::new(&runtime_root)
            .status(operation_id)
            .unwrap()
            .state,
        OperationState::Cancelled
    );

    let registry = FileOperationRegistry::new(&runtime_root);
    let interrupted = registry
        .register_with_request(
            "goals:jira-export",
            json!({
                "refine_dir": refine_dir.clone(),
                "target_root": temp_root.clone(),
                "selection": {"selected_ids": ["GOAL1"], "exclude_ids": [], "filter": {}}
            }),
        )
        .unwrap();
    registry
        .finish(&interrupted.id, OperationState::Interrupted)
        .unwrap();
    let recovered = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/goals/export/jira/{}/retry", interrupted.id),
        body: Some(json!({})),
    });
    assert_eq!(recovered.status, 202, "{:#}", recovered.body);
    let recovered_id = recovered.body["operation"]["id"].as_str().unwrap();
    let recovered_operation =
        wait_for_operation_status(&registry, recovered_id, OperationState::Succeeded);
    assert_eq!(recovered_operation.result["export"]["goal_count"], 1);
    assert_eq!(recovered_operation.request["recovery_of"], interrupted.id);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_goal_detail_exposes_failed_feature_blocking_notice() {
    let temp_root = unique_temp_dir("http-goal-feature-blocking-notice");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL1").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL2").unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });

    assert_eq!(show.status, 200);
    assert_eq!(
        show.body["goal"]["feature_blocking_notice"]["feature_id"],
        "FEA1"
    );
    assert_eq!(
        show.body["goal"]["feature_blocking_notice"]["blocked_goal_ids"],
        json!(["GOAL2"])
    );
    assert!(
        show.body["goal"]["feature_blocking_notice"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("Submit a recovery round")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_edits_notes_and_deletes_goal() {
    let temp_root = unique_temp_dir("http-edit-note-delete");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Original"})),
    });

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: Some(json!({"name": "Renamed", "priority": "high"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["goal"]["name"], "Renamed");
    assert_eq!(edit.body["goal"]["priority"], "high");

    let note = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals/GOAL1/notes".to_string(),
        body: Some(json!({"author": "Reviewer", "body": "Needs context"})),
    });
    assert_eq!(note.status, 200);
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"body\": \"Needs context\""));
    let written_goal = serde_json::from_str::<serde_json::Value>(&written).unwrap();
    let note_id = written_goal["notes"][0]["id"].as_str().unwrap();

    let edited_note = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: Some(json!({
            "notes": [{
                "id": note_id,
                "author": "Reviewer",
                "body": "Updated context",
                "created": written_goal["notes"][0]["created"].clone()
            }]
        })),
    });
    assert_eq!(edited_note.status, 200);
    let edited_detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(
        edited_detail.body["goal"]["notes"][0]["body"],
        "Updated context"
    );

    let deleted_note = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: Some(json!({"notes": []})),
    });
    assert_eq!(deleted_note.status, 200);
    let deleted_detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(deleted_detail.body["goal"]["notes"], json!([]));

    let delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(delete.status, 200);
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());

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
        path: "/work/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Round Goal"})),
    });

    let append = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals/GOAL1/rounds".to_string(),
        body: Some(json!({"reporter": "Reporter", "prompt": "Target"})),
    });
    assert_eq!(append.status, 200);
    assert_eq!(append.body["goal"]["round_count"], 1);

    let edit = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/work/goals/GOAL1/rounds/latest".to_string(),
        body: Some(json!({"reporter": "Reviewer", "assignee": "Reviewer", "prompt": "Revised"})),
    });
    assert_eq!(edit.status, 200);
    assert_eq!(edit.body["goal"]["reporter"], "Reviewer");
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"prompt\": \"Revised\""));

    let detail = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(detail.body["goal"]["round_count"], 1);
    assert_eq!(detail.body["goal"]["rounds"][0]["reporter"], "Reviewer");
    assert_eq!(detail.body["goal"]["rounds"][0]["assignee"], "Reviewer");
    assert_eq!(detail.body["goal"]["rounds"][0]["prompt"], "Revised");

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
fn web_server_appends_and_reads_goal_round_logs() {
    let temp_root = unique_temp_dir("http-goal-round-logs");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root);
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Logged Goal"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/rounds".to_string(),
        body: Some(json!({"reporter": "Reporter", "prompt": "Target"})),
    });
    let activity_before_logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity?goal_id=GOAL1".to_string(),
        body: None,
    });
    assert_eq!(activity_before_logs.status, 200);
    assert_eq!(activity_before_logs.body["page"]["total"], 0);

    let append = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/rounds/0/logs".to_string(),
        body: Some(json!({
            "severity": "info",
            "category": "state",
            "actor": "refine",
            "message": "Workflow status changed: backlog -> todo"
        })),
    });
    assert_eq!(append.status, 200);
    assert!(refine_dir.join("goals/GO/AL1/logs.jsonl").exists());

    let logs = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOAL1/logs".to_string(),
        body: None,
    });
    assert_eq!(logs.status, 200);
    assert_eq!(logs.body["round_log_count"], 1);
    assert_eq!(
        logs.body["logs"][0]["message"],
        "Workflow status changed: backlog -> todo"
    );
    let activity = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity?goal_id=GOAL1".to_string(),
        body: None,
    });
    assert_eq!(activity.status, 200);
    assert_eq!(activity.body["page"]["total"], 1);
    assert_eq!(
        activity.body["activity"][0]["message"],
        "Workflow status changed: backlog -> todo"
    );
    assert_eq!(activity.body["activity"][0]["goal_id"], "GOAL1");

    let evaluation = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/goals/GOAL1/rounds/latest/evaluation".to_string(),
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
        path: "/api/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(detail.status, 200);
    assert_eq!(detail.body["goal"]["rounds"][0]["rule_state"], "failed");
    assert_eq!(
        detail.body["goal"]["rounds"][0]["governance_message"],
        "Governance found a product concern."
    );
    assert_eq!(detail.body["goal"]["rounds"][0]["quality_state"], "failed");
    assert_eq!(
        detail.body["goal"]["rounds"][0]["quality_message"],
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
        path: "/work/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Goal One"})),
    });

    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);
    assert_eq!(create_feature.body["feature"]["id"], "FEA1");

    let add_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals".to_string(),
        body: Some(json!({"goal_id": "GOAL1"})),
    });
    assert_eq!(add_goal.status, 200);
    assert_eq!(add_goal.body["goal_ids"], json!(["GOAL1"]));

    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(show.status, 200);
    assert_eq!(show.body["goal_ids"], json!(["GOAL1"]));
    assert_eq!(show.body["feature"]["goal_ids"], json!(["GOAL1"]));
    assert_eq!(show.body["feature"]["goal_count"], 1);
    assert_eq!(show.body["feature"]["goals"][0]["id"], "GOAL1");

    let unorder_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/unorder".to_string(),
        body: None,
    });
    assert_eq!(unorder_goal.status, 200);
    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(
        show.body["feature"]["goals"][0]["feature_order"],
        json!(null)
    );

    let order_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/order".to_string(),
        body: None,
    });
    assert_eq!(order_goal.status, 200);
    let show = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(show.body["feature"]["goals"][0]["feature_order"], json!(1));

    let remove_goal = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(remove_goal.status, 200);
    assert_eq!(remove_goal.body["goal_ids"], json!([]));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_feature_goal_authoring_is_one_policy_driven_api_operation() {
    let temp_root = unique_temp_dir("http-feature-goal-authoring");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    assert_eq!(
        server
            .handle(ApiRequest {
                method: "POST".to_string(),
                path: "/api/features".to_string(),
                body: Some(json!({"id": "FEA1", "name": "Feature One"})),
            })
            .status,
        201
    );

    let first = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "name": "Foundation",
            "prompt": "Build the foundation",
            "reporter": "Buddy",
            "priority": "low",
            "placement": "first"
        })),
    });
    assert_eq!(first.status, 201, "{:#}", first.body);
    let first_id = first.body["goal"]["id"].as_str().unwrap().to_string();
    assert_eq!(first.body["goal"]["feature_order"], 1);

    let second = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "name": "Feature UI",
            "prompt": "Bind the inline composer",
            "reporter": "Buddy",
            "priority": "high",
            "placement": {"after": first_id}
        })),
    });
    assert_eq!(second.status, 201, "{:#}", second.body);
    let second_id = second.body["goal"]["id"].as_str().unwrap().to_string();
    assert_eq!(second.body["goal"]["feature_order"], 2);

    let duplicate = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "prompt": "Bind the inline composer",
            "reporter": "Buddy",
            "priority": "low",
            "placement": "unordered"
        })),
    });
    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.body["error"]["code"], "duplicate_goal");
    assert_eq!(
        duplicate.body["error"]["duplicate"]["match"]["id"],
        second_id
    );

    let goal_path = refine_dir
        .join("goals")
        .join(&second_id[..2])
        .join(&second_id[2..])
        .join("goal.json");
    let mut review_goal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    review_goal["status"] = json!("review");
    fs::write(&goal_path, serde_json::to_vec_pretty(&review_goal).unwrap()).unwrap();

    let shown = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features/FEA1".to_string(),
        body: None,
    });
    let review = shown.body["feature"]["goals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|goal| goal["id"] == second_id)
        .unwrap();
    assert_eq!(review["feature_authoring"]["editable"], true);

    let edited = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "goal_id": second_id,
            "name": "Reviewed UI",
            "prompt": "Revise while the Goal is in review",
            "reporter": "Buddy",
            "priority": "medium",
            "placement": "first"
        })),
    });
    assert_eq!(edited.status, 200, "{:#}", edited.body);
    assert_eq!(edited.body["goal"]["status"], "review");
    assert_eq!(edited.body["goal"]["feature_order"], 1);
    assert_eq!(edited.body["goal"]["name"], "Reviewed UI");

    let invalid = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/author".to_string(),
        body: Some(json!({
            "prompt": "Invalid placement",
            "reporter": "Buddy",
            "priority": "low",
            "placement": {"after": "MISSING"}
        })),
    });
    assert_eq!(invalid.status, 400);
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .list_goal_summaries()
            .unwrap()
            .len(),
        2
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_reorders_and_moves_feature_workflow() {
    let temp_root = unique_temp_dir("http-feature-reorder-move");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [("GOAL1", "Goal One"), ("GOAL2", "Goal Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for goal_id in ["GOAL1", "GOAL2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/goals".to_string(),
            body: Some(json!({"goal_id": goal_id})),
        });
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/work/features/FEA1/goals/{goal_id}/order"),
            body: None,
        });
    }

    let reorder = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL2/reorder".to_string(),
        body: Some(json!({"order": 1})),
    });
    assert_eq!(reorder.status, 200);
    assert_eq!(reorder.body["goal_ids"], json!(["GOAL2", "GOAL1"]));

    let reorder_before = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/reorder".to_string(),
        body: Some(json!({"before": "GOAL2"})),
    });
    assert_eq!(reorder_before.status, 200);
    assert_eq!(reorder_before.body["goal_ids"], json!(["GOAL1", "GOAL2"]));

    let reorder_after = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/reorder".to_string(),
        body: Some(json!({"after": "GOAL2"})),
    });
    assert_eq!(reorder_after.status, 200);
    assert_eq!(reorder_after.body["goal_ids"], json!(["GOAL2", "GOAL1"]));

    let move_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/move".to_string(),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(move_feature.status, 200);
    assert_eq!(move_feature.body["rollup"]["status"], "todo");
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_updates_feature_metadata_and_runs_goal_actions() {
    let temp_root = unique_temp_dir("http-feature-goal-actions");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [
        ("GOAL1", "Verify Goal"),
        ("GOAL2", "Retry Quality"),
        ("GOAL3", "Retry Merge"),
        ("GOAL4", "Submit Merge"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
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

    let goal_actions = FileWorkItemService::new(&refine_dir);
    goal_actions
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    for status in [
        GoalStatus::InProgress,
        GoalStatus::ReadyMerge,
        GoalStatus::Build,
        GoalStatus::Qa,
    ] {
        goal_actions
            .advance_automated_goal_status("GOAL1", status)
            .unwrap();
    }
    let verified = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/verify".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(verified.status, 200);
    assert_eq!(verified.body["ok"], true);
    assert_eq!(verified.body["goal"]["status"], "done");

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL2", "GOAL3"],
            "update": {"status": "failed"}
        })),
    });
    let retry_quality = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL2/retry-quality".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(retry_quality.status, 200);
    assert_eq!(retry_quality.body["goal"]["status"], "qa");

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL4/start".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(started.status, 200);
    assert_eq!(started.body["goal"]["status"], "todo");
    FileWorkItemService::new(&refine_dir)
        .advance_automated_goal_status("GOAL4", GoalStatus::InProgress)
        .unwrap();
    let submitted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL4/submit-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(submitted.status, 200);
    assert_eq!(submitted.body["goal"]["status"], "ready-merge");
    let submitted_again = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL4/submit-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(submitted_again.status, 200);
    assert_eq!(submitted_again.body["goal"]["status"], "ready-merge");

    let retry_merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL3/retry-merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(retry_merge.status, 200);
    assert_eq!(retry_merge.body["goal"]["status"], "ready-merge");

    let merge = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL3/merge".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(merge.status, 503);
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL3")
            .unwrap()
            .goal
            .status,
        GoalStatus::ReadyMerge
    );

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
    for (id, name) in [("GOAL1", "Goal One"), ("GOAL2", "Goal Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for goal_id in ["GOAL1", "GOAL2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/goals".to_string(),
            body: Some(json!({"goal_id": goal_id})),
        });
    }

    let goal_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals/GOAL1/cancel".to_string(),
        body: None,
    });
    assert_eq!(goal_cancel.status, 200);
    assert_eq!(goal_cancel.body["goal"]["status"], "cancelled");

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .register(ManagedProcess {
            id: "agent-goal2".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("agent".to_string()),
            details: Some("working on GOAL2".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "now".to_string(),
            exit_code: None,
        })
        .unwrap();
    let operation = FileOperationRegistry::new(&runtime_root)
        .register("feature FEA1 goal GOAL2")
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
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_accepts_static_ui_api_aliases_for_work_routes() {
    let temp_root = unique_temp_dir("http-api-aliases");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let create_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Goal One"})),
    });
    assert_eq!(create_goal.status, 201);
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);

    let add_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(add_goal.status, 200);
    assert_eq!(add_goal.body["goal_ids"], json!(["GOAL1"]));

    let workflow = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/workflow".to_string(),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(workflow.status, 200);
    assert_eq!(workflow.body["rollup"]["status"], "todo");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/cancel".to_string(),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["goal"]["status"], "cancelled");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_accepts_static_ui_bulk_api_aliases() {
    let temp_root = unique_temp_dir("http-bulk-api-aliases");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [("GOAL1", "Bulk One"), ("GOAL2", "Bulk Two")] {
        let create = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
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
    let create_second_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA2", "name": "Bulk Feature Two"})),
    });
    assert_eq!(create_second_feature.status, 201);

    let bulk_status = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL2"],
            "update": {"status": "todo"}
        })),
    });
    assert_eq!(bulk_status.status, 200);
    assert_eq!(bulk_status.body["updated"], 2);

    let bulk_assign = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/bulk".to_string(),
        body: Some(json!({"selected_ids": ["GOAL1", "GOAL2"]})),
    });
    assert_eq!(bulk_assign.status, 200);
    assert_eq!(bulk_assign.body["updated"], 2);
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"feature_id\": \"FEA1\"")
    );

    let bulk_feature_update = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["FEA1", "FEA2"],
            "update": {"reporter": "Feature Reporter"}
        })),
    });
    assert_eq!(bulk_feature_update.status, 200);
    assert_eq!(bulk_feature_update.body["updated"], 2);
    assert!(
        fs::read_to_string(refine_dir.join("features/FE/A2/feature.json"))
            .unwrap()
            .contains("\"reporter\": \"Feature Reporter\"")
    );

    let bulk_feature_delete = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/bulk/delete".to_string(),
        body: Some(json!({"selected_ids": ["FEA2"]})),
    });
    assert_eq!(bulk_feature_delete.status, 200);
    assert_eq!(bulk_feature_delete.body["deleted"], 1);
    assert!(!refine_dir.join("features/FE/A2/feature.json").exists());

    let bulk_delete = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk/delete".to_string(),
        body: Some(json!({"selected_ids": ["GOAL1"]})),
    });
    assert_eq!(bulk_delete.status, 200);
    assert_eq!(bulk_delete.body["deleted"], 1);
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(refine_dir.join("goals/GO/AL2/goal.json").exists());

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
fn web_server_parses_and_persists_imported_goals_with_feature_destination() {
    let temp_root = unique_temp_dir("http-import-persist");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let parsed = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/csv/parse".to_string(),
        body: Some(json!({
            "text": "name,prompt,reporter,priority\nCSV Goal,Implement target state,QA,high\n"
        })),
    });
    assert_eq!(parsed.status, 200);
    assert_eq!(parsed.body["drafts"][0]["name"], "CSV Goal");
    assert_eq!(parsed.body["drafts"][0]["priority"], "high");

    let persisted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/persist".to_string(),
        body: Some(json!({
            "new_feature_name": "Imported Feature",
            "drafts": [{
                "name": "Imported Goal",
                "prompt": "Target state",
                "reporter": "QA",
                "priority": "high"
            }]
        })),
    });
    assert_eq!(persisted.status, 201);
    assert_eq!(persisted.body["count"], 1);
    assert_eq!(persisted.body["feature"]["name"], "Imported Feature");
    let goal_id = persisted.body["goals"][0]["id"].as_str().unwrap();
    let feature_id = persisted.body["feature"]["id"].as_str().unwrap();

    let goal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/goals/{goal_id}"),
        body: None,
    });
    assert_eq!(goal.status, 200);
    assert_eq!(goal.body["goal"]["priority"], "high");
    assert_eq!(goal.body["goal"]["reporter"], "QA");
    assert_eq!(goal.body["goal"]["round_count"], 1);
    assert_eq!(goal.body["goal"]["feature_id"], feature_id);
    assert_eq!(goal.body["goal"]["feature_order"], json!(null));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_parses_import_csv_in_background() {
    let temp_root = unique_temp_dir("http-import-csv-background");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/csv/parse".to_string(),
        body: Some(json!({
            "background": true,
            "text": "name,prompt,reporter,priority\nBackground CSV,Implement target state,QA,high\n"
        })),
    });
    assert_eq!(started.status, 202);
    let operation_id = started.body["operation"]["id"].as_str().unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = wait_for_operation_status(&registry, operation_id, OperationState::Succeeded);
    let result = operation.result;
    assert_eq!(result["http_status"], 200);
    assert_eq!(result["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(result["drafts"][0]["name"], "Background CSV");
    assert_eq!(result["drafts"][0]["priority"], "high");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_background_feature_import_promotes_all_instant_backlog_goals() {
    let temp_root = unique_temp_dir("http-import-feature-promote-all");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/persist".to_string(),
        body: Some(json!({
            "background": true,
            "new_feature_name": "Instant Feature",
            "drafts": [
                {
                    "name": "First imported Goal",
                    "prompt": "First target",
                    "priority": "high"
                },
                {
                    "name": "Second imported Goal",
                    "prompt": "Second target",
                    "priority": "medium"
                },
                {
                    "name": "Third imported Goal",
                    "prompt": "Third target",
                    "priority": "low"
                }
            ]
        })),
    });
    assert_eq!(started.status, 202);
    let operation_id = started.body["operation"]["id"].as_str().unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = wait_for_operation_status(&registry, operation_id, OperationState::Succeeded);
    let result = operation.result;
    assert_eq!(result["http_status"], 201);
    assert_eq!(result["count"], 3);
    assert_eq!(result["promoted"], 3);
    let goals = result["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 3);
    assert!(goals.iter().all(|goal| goal["status"] == "todo"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_extracts_plan_drafts_from_chat_session_context() {
    let temp_root = unique_temp_dir("http-import-plan-chat-context");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let plan_feature = json!({
        "feature": {
            "name": "Chat Context Feature",
            "description": "Feature extracted from persisted Plan chat context.",
            "goals": [{
                "name": "Use persisted chat transcript",
                "prompt": "Draft Feature extracts from the stored Plan chat transcript.",
                "priority": "high"
            }]
        }
    })
    .to_string();
    write_fake_provider(&refine_dir, "smoke-ai", 0, &plan_feature);
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/chat/start".to_string(),
        body: Some(json!({"purpose": "plan", "provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201);
    let session_id = started.body["session_id"].as_str().unwrap().to_string();

    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/input"),
        body: Some(json!({"text": "Plan the chat-context feature."})),
    });
    assert_eq!(input.status, 200);
    wait_for_chat_read_line(&server, &session_id, "Chat Context Feature");
    let fallback_feature = json!({
        "feature": {
            "name": "Fallback Feature",
            "goals": [{
                "name": "Fallback goal",
                "prompt": "Fallback target"
            }]
        }
    })
    .to_string();

    let extracted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/extract".to_string(),
        body: Some(json!({
            "purpose": "plan",
            "chat_session_id": session_id,
            "text": fallback_feature
        })),
    });
    assert_eq!(extracted.status, 200);
    assert_eq!(
        extracted.body["feature_destination"]["newName"],
        "Chat Context Feature"
    );
    assert_eq!(extracted.body["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(
        extracted.body["drafts"][0]["name"],
        "Use persisted chat transcript"
    );
    assert_eq!(extracted.body["source"], "input");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_fails_background_plan_extraction_without_goal_drafts() {
    let temp_root = unique_temp_dir("http-import-plan-empty-background");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/extract".to_string(),
        body: Some(json!({
            "purpose": "plan",
            "background": true,
            "text": "[]"
        })),
    });
    assert_eq!(started.status, 202);
    let operation_id = started.body["operation"]["id"].as_str().unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = wait_for_operation_status(&registry, operation_id, OperationState::Failed);
    let error = operation.error.unwrap();
    assert_eq!(error["code"], "invalid_input");
    assert_eq!(
        error["message"],
        "Plan Draft extraction did not return any Goal drafts"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_extracts_exactly_one_plan_goal_without_a_feature_destination() {
    let temp_root = unique_temp_dir("http-import-plan-goal");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_fake_provider(
        &refine_dir,
        "smoke-ai",
        0,
        &json!({
            "feature": {
                "name": "Must not escape",
                "goals": [{
                    "name": "One planned Goal",
                    "prompt": "Implement one reviewable slice from the Plan transcript.",
                    "priority": "medium"
                }]
            }
        })
        .to_string(),
    );
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var(
            "REFINE_SMOKE_AI_PATH",
            refine_dir.join("provider-bin/smoke-ai").to_str().unwrap(),
        );
    }
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());

    let extracted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/extract".to_string(),
        body: Some(json!({
            "purpose": "plan_goal",
            "provider": "smoke-ai",
            "text": "Plan one independently actionable implementation slice."
        })),
    });

    assert_eq!(extracted.status, 200, "{}", extracted.body);
    assert_eq!(extracted.body["purpose"], "plan_goal");
    assert_eq!(extracted.body["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(extracted.body["drafts"][0]["name"], "One planned Goal");
    assert!(extracted.body.get("feature_destination").is_none());

    let through_mcp = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/mcp".to_string(),
        body: Some(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "refine_draft_goal",
                "arguments": {
                    "provider": "smoke-ai",
                    "text": "Plan one independently actionable implementation slice."
                }
            }
        })),
    });
    assert_eq!(through_mcp.status, 200, "{}", through_mcp.body);
    assert_eq!(through_mcp.body["result"]["isError"], false);
    let mcp_drafts = through_mcp.body["result"]["structuredContent"]["drafts"]
        .as_array()
        .unwrap();
    assert_eq!(mcp_drafts.len(), 1);
    assert_eq!(mcp_drafts[0]["name"], "One planned Goal");
    assert!(
        through_mcp.body["result"]["structuredContent"]
            .get("feature_destination")
            .is_none()
    );

    unsafe {
        match previous_smoke_ai {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_force_provider_plan_extraction_skips_structured_input_parse() {
    let temp_root = unique_temp_dir("http-import-plan-force-provider");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_fake_provider(
        &refine_dir,
        "smoke-ai",
        0,
        &json!({
            "feature": {
                "name": "Provider Extracted Feature",
                "goals": [{
                    "name": "Provider extracted goal",
                    "prompt": "The provider extracts implementation-ready drafts.",
                    "priority": "medium"
                }]
            }
        })
        .to_string(),
    );
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var(
            "REFINE_SMOKE_AI_PATH",
            refine_dir.join("provider-bin/smoke-ai").to_str().unwrap(),
        );
    }
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());

    let extracted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/extract".to_string(),
        body: Some(json!({
            "purpose": "plan",
            "provider": "smoke-ai",
            "force_provider": true,
            "text": "[]"
        })),
    });
    assert_eq!(extracted.status, 200);
    assert_eq!(extracted.body["source"], "provider");
    assert_eq!(
        extracted.body["feature_destination"]["newName"],
        "Provider Extracted Feature"
    );
    assert_eq!(extracted.body["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(
        extracted.body["drafts"][0]["name"],
        "Provider extracted goal"
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
fn daemon_agent_automation_loop_executes_todo_goals_without_manual_request() {
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
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "target_app_build_command": "printf build-ok",
            "allowed_commands": "printf"
        }))
        .unwrap();

    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Loop schedulable"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/transition".to_string(),
        body: Some(json!({"status": "todo"})),
    });

    let daemon = LocalHttpDaemon {
        server: server.clone(),
        static_root: None,
    };
    let automation_loop = daemon.start_agent_automation_loop(Duration::from_millis(25));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let show = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/goals/GOAL1".to_string(),
            body: None,
        });
        assert_eq!(show.status, 200);
        if show.body["goal"]["status"] == "review" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "automation loop did not execute GOAL1 before timeout: {}",
            show.body["goal"]["status"]
        );
        thread::sleep(Duration::from_millis(25));
    }
    automation_loop.stop_for_test();

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    assert!(state.contains("\"goal_id\": \"GOAL1\""));
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
fn web_server_cancels_background_import_persist_and_rolls_back_created_goals() {
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
                "prompt": format!("{prefix} prompt {index:03}"),
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
        let goals = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/goals?limit=1000&node=current&q={prefix}"),
            body: None,
        });
        assert_eq!(goals.status, 200);
        let total = goals.body["page"]["total"].as_u64().unwrap();
        if total == 0 {
            break;
        }
        assert!(
            Instant::now() < projection_deadline,
            "cancelled import left {total} matching Goal records"
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
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Cached Goal"})),
    });
    let rebuilt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/cache/rebuild".to_string(),
        body: Some(json!({"background": true})),
    });
    assert_eq!(rebuilt.status, 200);
    assert_eq!(rebuilt.body["goals"], 1);
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
    let goal_dir = refine_dir.join("goals").join("GO").join("AL1");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&goal_dir).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "one\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Change-linked Goal",
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
    git(&temp_root, &["commit", "-am", "GOAL1 update app"]).unwrap();

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
    assert_eq!(changes.body["changes"][0]["subject"], "GOAL1 update app");
    assert_eq!(changes.body["changes"][0]["goal_id"], "GOAL1");
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
    assert_eq!(no_repo.status, 202);
    let no_repo =
        wait_for_project_sync_operation(&runtime_root, &no_repo, OperationState::Succeeded);
    assert_eq!(no_repo.result["git_sync"]["attempted"], false);
    assert_eq!(no_repo.result["git_sync"]["pulled"], false);
    assert_eq!(
        no_repo.result["git_sync"]["detail"],
        "Target app is not a Git repository."
    );

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
    let missing_upstream = wait_for_project_sync_operation(
        &runtime_root,
        &missing_upstream,
        OperationState::Succeeded,
    );
    assert_eq!(missing_upstream.result["git_sync"]["attempted"], true);
    assert_eq!(missing_upstream.result["git_sync"]["committed"], true);
    assert_eq!(missing_upstream.result["git_sync"]["pushed"], false);
    assert!(
        missing_upstream.result["git_sync"]["detail"]
            .as_str()
            .unwrap()
            .contains("Git remote origin is not configured")
    );
    assert!(!app_root.join(".refine").exists());
    assert_eq!(git_stdout(&app_root, &["branch", "--show-current"]), "main");
    assert!(
        git(
            &app_root,
            &["show-ref", "--verify", "refs/heads/refine/state"]
        )
        .is_ok()
    );
    assert!(git(&app_root, &["check-ignore", "-q", ".refine/probe.json"]).is_ok());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_returns_while_repository_worker_is_busy() {
    let temp_root = unique_temp_dir("http-project-sync-nonblocking");
    let app_root = temp_root.join("app");
    let refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    git(&app_root, &["init", "-b", "main"]).unwrap();
    git(&app_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&app_root, &["config", "user.name", "Test User"]).unwrap();
    git(&app_root, &["commit", "--allow-empty", "-m", "initial"]).unwrap();

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
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let started = Instant::now();
    let response = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });

    assert_eq!(response.status, 202, "{:#}", response.body);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "project sync request waited for the repository lock"
    );
    release_tx.send(()).unwrap();
    lock_thread.join().unwrap();
    let operation =
        wait_for_project_sync_operation(&runtime_root, &response, OperationState::Succeeded);
    assert_eq!(operation.result["git_sync"]["attempted"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_ignores_refine_runtime_noise() {
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
    let refine_dir = refine_dir_for_target_root(&app_root).unwrap();
    fs::create_dir_all(refine_dir.join("runtime/processes")).unwrap();
    fs::write(
        refine_dir.join("runtime/processes/local.json"),
        r#"{"id":"local","owner":"maintenance","pid":null,"state":"running","label":"local","details":"runtime noise","started_at":"now"}"#,
    )
    .unwrap();
    fs::write(seed.join("remote.txt"), "remote\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "remote update"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    let runtime_root = temp_root.join("run/8080");
    server.runtime_root = Some(runtime_root.clone());
    let sync = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    let sync = wait_for_project_sync_operation(&runtime_root, &sync, OperationState::Succeeded);
    assert_eq!(sync.result["git_sync"]["attempted"], true);
    assert_eq!(sync.result["git_sync"]["branch"], "refine/state");
    assert_eq!(sync.result["git_sync"]["pulled"], false);
    assert!(!app_root.join("remote.txt").exists());
    assert!(refine_dir.join("runtime/processes/local.json").exists());
    assert!(!app_root.join(".refine").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_ignores_dirty_user_worktree() {
    let temp_root = unique_temp_dir("http-project-sync-dirty");
    let (seed, app_root) = seeded_remote_clone(&temp_root);
    fs::write(seed.join("remote.txt"), "remote\n").unwrap();
    git(&seed, &["add", "remote.txt"]).unwrap();
    git(&seed, &["commit", "-m", "remote update"]).unwrap();
    git(&seed, &["push", "origin", "main"]).unwrap();
    fs::write(app_root.join("local.txt"), "local dirty\n").unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    let runtime_root = temp_root.join("run/8080");
    server.runtime_root = Some(runtime_root.clone());
    let sync = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    let sync = wait_for_project_sync_operation(&runtime_root, &sync, OperationState::Succeeded);
    assert_eq!(sync.result["git_sync"]["attempted"], true);
    assert_eq!(sync.result["git_sync"]["branch"], "refine/state");
    assert!(!app_root.join("remote.txt").exists());
    assert_eq!(
        fs::read_to_string(app_root.join("local.txt")).unwrap(),
        "local dirty\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_project_sync_does_not_rebase_or_push_application_branches() {
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
    let runtime_root = temp_root.join("run/8080");
    server.runtime_root = Some(runtime_root.clone());
    let sync = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/sync".to_string(),
        body: Some(json!({})),
    });
    let sync = wait_for_project_sync_operation(&runtime_root, &sync, OperationState::Succeeded);
    assert_eq!(sync.result["git_sync"]["attempted"], true);
    assert_eq!(sync.result["git_sync"]["pulled"], false);
    assert_eq!(sync.result["git_sync"]["pushed"], true);
    assert_eq!(sync.result["git_sync"]["branch"], "refine/state");
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
fn web_server_manages_nodes_and_transfers_goal_ownership() {
    let temp_root = unique_temp_dir("http-node-transfer");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    for (id, name) in [
        ("GOAL1", "Transfer One"),
        ("GOAL2", "Transfer Two"),
        ("GOAL3", "Stay Default"),
    ] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
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
    assert!(runtime_root.join("active-node.json").exists());
    assert!(!refine_dir.join("active-node.json").exists());

    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-goals".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL2"],
            "target_node_id": "remote-qa"
        })),
    });
    assert_eq!(transfer.status, 200);
    assert_eq!(transfer.body["updated"], 2);
    let current_node_goals = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?node=current".to_string(),
        body: None,
    });
    assert_eq!(current_node_goals.status, 200);
    assert_eq!(current_node_goals.body["page"]["total"], 2);
    assert_eq!(
        current_node_goals.body["goals"][0]["node_display_name"],
        "Remote QA"
    );
    let all_node_goals = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_node_goals.status, 200);
    assert_eq!(all_node_goals.body["page"]["total"], 3);
    let current_dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(current_dashboard.status, 200);
    assert_eq!(current_dashboard.body["node_filter"], "current");
    assert_eq!(current_dashboard.body["active_node_id"], "remote-qa");
    assert_eq!(current_dashboard.body["counts"]["backlog"], 2);
    let all_dashboard = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_dashboard.status, 200);
    assert_eq!(all_dashboard.body["node_filter"], "all");
    assert_eq!(all_dashboard.body["counts"]["backlog"], 3);
    let goal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(goal.body["goal"]["node_id"], "remote-qa");

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
fn web_server_transfers_feature_ownership_as_a_unit() {
    let temp_root = unique_temp_dir("http-feature-node-transfer");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root);

    let create_node = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        body: Some(json!({"id": "remote-node", "display_name": "Remote Node"})),
    });
    assert_eq!(create_node.status, 200);
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Transfer Feature"})),
    });
    assert_eq!(create_feature.status, 201);
    for (id, name) in [("GOAL1", "Feature One"), ("GOAL2", "Feature Two")] {
        let goal = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
        assert_eq!(goal.status, 201);
        let assign = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/features/FEA1/goals/{id}"),
            body: None,
        });
        assert_eq!(assign.status, 200);
    }

    let direct_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-goals".to_string(),
        body: Some(json!({
            "item_id": "GOAL1",
            "target_node_id": "remote-node"
        })),
    });
    assert_eq!(direct_goal.status, 409);
    assert!(
        direct_goal.body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("transfer the Feature instead"),
        "{direct_goal:#?}"
    );

    let bulk_transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-features".to_string(),
        body: Some(json!({
            "selected_ids": ["FEA1"],
            "target_node_id": "remote-node"
        })),
    });
    assert_eq!(bulk_transfer.status, 200);
    assert_eq!(bulk_transfer.body["updated"], 3);
    assert_eq!(bulk_transfer.body["ids"], json!(["FEA1", "GOAL1", "GOAL2"]));

    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/transfer".to_string(),
        body: Some(json!({"target_node_id": "remote-node"})),
    });
    assert_eq!(transfer.status, 200);
    assert_eq!(transfer.body["updated"], 3);
    assert_eq!(transfer.body["ids"], json!(["FEA1", "GOAL1", "GOAL2"]));
    let feature = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(feature.body["feature"]["node_id"], "remote-node");
    for id in ["GOAL1", "GOAL2"] {
        let goal = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/goals/{id}"),
            body: None,
        });
        assert_eq!(goal.body["goal"]["node_id"], "remote-node");
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_cluster_operations_over_nodes() {
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
    let registered_node = registered.body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(registered_node["ssh_host"], "example.com");
    assert_eq!(registered_node["ssh_user"], "deploy");
    assert_eq!(
        registered_node["ssh_identity_path"],
        "~/.ssh/refine_ed25519"
    );
    assert!(!refine_dir.join("cluster.json").exists());

    let disabled = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/cluster/nodes/node-1".to_string(),
        body: Some(json!({"enabled": false, "ssh_port": 2222})),
    });
    assert_eq!(disabled.status, 200);
    let disabled_node = disabled.body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(disabled_node["enabled"], false);
    assert_eq!(disabled_node["ssh_port"], 2222);

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
        bootstrap.body["cluster"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "node-1")
            .unwrap()["health"]["status"],
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
    fs::write(refine_dir.join("refine.json"), "{}").unwrap();
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
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(temp_root.join("child")).unwrap();
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
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
                goal_id: None,
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
fn native_sse_streams_projected_goal_round_logs() {
    let mut server = server_with_projection();
    server.projection.activity.insert(
        "round-log:GOAL1:0:0".to_string(),
        crate::tools::product::project_state::ActivitySummaryProjection {
            entry: ActivityEntry {
                id: "round-log:GOAL1:0:0".to_string(),
                datetime: "2026-07-21T12:00:00Z".to_string(),
                severity: "info".to_string(),
                category: "agent".to_string(),
                message: "Agent edited the implementation".to_string(),
                goal_id: Some("GOAL1".to_string()),
                actor: Some("codex".to_string()),
                details: None,
                actions: Vec::new(),
            },
            searchable_text: "Agent edited the implementation".to_string(),
        },
    );
    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };

    let events = daemon.server_sent_events("events").unwrap();

    assert!(events.contains("event: goal_log_added"));
    assert!(events.contains("Agent edited the implementation"));
    assert!(events.contains(r#""goal_id":"GOAL1""#));
}

#[test]
fn goal_log_activity_page_returns_the_newest_entries() {
    let mut server = server_with_projection();
    for index in 0..205 {
        let id = format!("round-log:GOAL1:0:{index:03}");
        server.projection.activity.insert(
            id.clone(),
            crate::tools::product::project_state::ActivitySummaryProjection {
                entry: ActivityEntry {
                    id,
                    datetime: format!("2026-07-21T12:{:02}:{:02}Z", index / 60, index % 60),
                    severity: "info".to_string(),
                    category: "agent".to_string(),
                    message: format!("Goal log {index:03}"),
                    goal_id: Some("GOAL1".to_string()),
                    actor: Some("codex".to_string()),
                    details: None,
                    actions: Vec::new(),
                },
                searchable_text: format!("Goal log {index:03}"),
            },
        );
    }

    let result = server.projection.list_activity(ActivityProjectionQuery {
        page: PageRequest {
            limit: 200,
            offset: 0,
            sort: "datetime".to_string(),
            dir: "desc".to_string(),
        },
        goal_id: Some("GOAL1".to_string()),
        ..ActivityProjectionQuery::default()
    });

    assert_eq!(result.total, 205);
    assert_eq!(result.activity.len(), 200);
    assert_eq!(result.activity[0].message, "Goal log 204");
    assert_eq!(result.activity[199].message, "Goal log 005");
}

#[test]
fn web_server_reads_and_cancels_runtime_operations() {
    let temp_root = unique_temp_dir("http-operations");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry.register("bulk_update_goals").unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{}", operation.id),
        body: None,
    });
    assert_eq!(status.status, 200, "{:#}", status.body);
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
    let claim_id = automation.claim("GOAL1").unwrap();
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
    assert_eq!(retry.body["execution"]["goal_id"], "GOAL1");
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
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
    let standalone_chat = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), None)
        .unwrap();
    let goal_chat = chat
        .start_with_options(
            ChatAttachment::Goal("GOALCHAT".to_string()),
            Some("smoke-ai"),
            Some("goal"),
        )
        .unwrap();
    let stopped_chat = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), None)
        .unwrap();
    chat.stop(&stopped_chat.id).unwrap();
    supervisor
        .register(ManagedProcess {
            id: "supervisor-context".to_string(),
            owner: ProcessOwner::Daemon,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("setsid".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    // Launch a real, long-lived agent process so it stays alive (and counted)
    // through the assertions below. A short-lived command would exit before the
    // summary call and be pruned by liveness recovery, racing the agent count.
    let launched_agent = supervisor
        .launch(crate::process::subprocess::ManagedProcessSpec {
            owner: crate::process::subprocess::ProcessOwner::Agent,
            command: if cfg!(windows) { "cmd" } else { "sleep" }.to_string(),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()]
            } else {
                vec!["30".to_string()]
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
            details: Some(json!({"goal_id": "GOALCTX", "round_idx": 1}).to_string()),
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
            details: Some(json!({"goal_id": "DONECTX", "round_idx": 1}).to_string()),
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
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["processes"][0]["kind"], "agent");
    assert_eq!(listed.body["runner_reachable"], false);
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
    let supervisor_context = listed_processes
        .iter()
        .find(|process| process["id"] == "supervisor-context")
        .unwrap();
    assert_eq!(supervisor_context["kind"], "daemon");
    assert_eq!(supervisor_context["actions"], json!(["terminate", "kill"]));
    assert_eq!(
        supervisor_context["management_actions"],
        json!(["pause_workflow"])
    );
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "exited-agent-context")
    );
    let agent_context = listed_processes
        .iter()
        .find(|process| process["id"] == "agent-context")
        .unwrap();
    assert_eq!(agent_context["goal_id"], "GOALCTX");
    assert_eq!(agent_context["round_idx"], 1);
    assert_eq!(agent_context["management_actions"], json!(["cancel_agent"]));
    let chat_context = listed_processes
        .iter()
        .find(|process| process["id"] == "chat-context")
        .unwrap();
    assert_eq!(chat_context["kind"], "chat");
    assert_eq!(chat_context["session_id"], "chat-context-session");
    assert_eq!(chat_context["mode"], "standalone");
    assert_eq!(chat_context["management_actions"], json!(["stop_chat"]));
    let standalone_context = listed_processes
        .iter()
        .find(|process| process["id"] == format!("chat-session-{}", standalone_chat.id))
        .unwrap();
    assert_eq!(standalone_context["kind"], "chat");
    assert_eq!(standalone_context["session_id"], standalone_chat.id);
    assert_eq!(standalone_context["mode"], "standalone");
    let goal_chat_context = listed_processes
        .iter()
        .find(|process| process["id"] == format!("chat-session-{}", goal_chat.id))
        .unwrap();
    assert_eq!(goal_chat_context["kind"], "chat");
    assert_eq!(goal_chat_context["session_id"], goal_chat.id);
    assert_eq!(goal_chat_context["mode"], "goal");
    assert_eq!(goal_chat_context["goal_id"], "GOALCHAT");
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
    assert_eq!(summary.body["process_count"], 7);
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
            .any(|process| process["goal_id"] == "GOALCTX")
    );
    assert_eq!(
        cached.runtime.supervisor.unwrap()["runner_reachable"],
        json!(false)
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
    supervisor
        .register(crate::process::subprocess::ManagedProcess {
            id: "stop-test".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: None,
            state: "running".to_string(),
            label: Some("stop".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/stop-test/stop".to_string(),
        body: None,
    });
    assert_eq!(stopped.status, 200);
    assert_eq!(stopped.body["stopped"], true);
    assert_eq!(stopped.body["process"]["id"], "stop-test");
    assert!(supervisor.inspect("stop-test").is_err());

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Pause workflow rollback", Some("GOAL-WORKFLOW"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL-WORKFLOW", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-WORKFLOW", GoalStatus::InProgress)
        .unwrap();
    let paused = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/workflow/pause".to_string(),
        body: Some(json!({"paused": true})),
    });
    assert_eq!(paused.status, 200);
    assert_eq!(paused.body["background_processes_stopped"], true);
    assert_eq!(paused.body["agents_paused"], true);
    let paused_supervisor = paused.body["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|process| process["id"] == "supervisor-context")
        .unwrap();
    assert_eq!(
        paused_supervisor["management_actions"],
        json!(["unpause_workflow"])
    );
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-WORKFLOW")
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );
    assert!(
        paused.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|work| work["status"] == "paused")
    );

    let resumed = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/workflow/pause".to_string(),
        body: Some(json!({"paused": false})),
    });
    assert_eq!(resumed.status, 200);
    assert_eq!(resumed.body["background_processes_stopped"], false);
    assert_eq!(resumed.body["agents_paused"], false);
    assert!(runtime_root.join("process-control.json").exists());
    let cached = FileProjectStateStore::new(&refine_dir)
        .load_projection_snapshot(&runtime_root.join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(
        cached.runtime.supervisor.unwrap()["agents_paused"],
        json!(false)
    );

    // Terminate the long-lived agent so the test leaves no orphaned process.
    let _ = supervisor.signal(&launched_agent.id, "terminate");

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
fn web_server_manages_quality_settings_and_checks() {
    let temp_root = unique_temp_dir("http-quality");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());

    let app_settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "target_app_test_commands": [
                {"command": "printf target-test-ok", "enabled": true},
                {"command": "printf skipped", "enabled": false}
            ]
        })),
    });
    assert_eq!(app_settings.status, 200);
    assert_eq!(
        app_settings.body["settings"]["target_app_test_command"],
        "printf target-test-ok"
    );

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
            "business_requirements": "Dashboard must render",
            "instructions": "Run focused checks"
        })),
    });
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["enabled"], "1");
    assert_eq!(saved.body["timing"], "post_build");
    assert_eq!(saved.body["configured"], true);

    let checks = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/quality/checks".to_string(),
        body: Some(json!({
            "owner_id": "GOAL1",
            "command": "printf quality-ok"
        })),
    });
    assert_eq!(checks.status, 200);
    assert_eq!(checks.body["ok"], true);
    assert_eq!(checks.body["result"]["owner_id"], "GOAL1");
    assert_eq!(checks.body["operation"]["owner"], "quality:GOAL1");
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

    let screenshots = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality/screenshots?owner_id=GOAL1".to_string(),
        body: None,
    });
    assert_eq!(screenshots.status, 200);
    assert_eq!(screenshots.body["owner_id"], "GOAL1");
    assert_eq!(screenshots.body["screenshot_count"], 0);
    assert!(
        screenshots.body["screenshots"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/quality".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert!(listed.body.get("regressions").is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_shares_supervisor_state_and_singleton_chat_session() {
    let temp_root = unique_temp_dir("http-supervisor-agent");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);
    server
        .settings_service(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();

    let goal_path = refine_dir.join("goals/GO/GOAL1/goal.json");
    fs::create_dir_all(goal_path.parent().unwrap()).unwrap();
    let write_goal_status = |status: &str| {
        fs::write(
            &goal_path,
            serde_json::to_vec_pretty(&json!({
                "id": "GOAL1",
                "name": "Live Supervisor state",
                "status": status,
                "created": "2026-07-22T12:00:00Z",
                "updated": Utc::now().to_rfc3339(),
                "priority": "low",
                "reporter": "test",
                "rounds": []
            }))
            .unwrap(),
        )
        .unwrap();
    };
    write_goal_status("review");

    let state = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/supervisor-agent".to_string(),
        body: None,
    });
    assert_eq!(state.status, 200);
    assert_eq!(state.body["supervisor_agent"]["lifecycle"], "idle");
    assert_eq!(state.body["supervisor_agent"]["health"], "healthy");
    assert_eq!(
        state.body["supervisor_agent"]["goal_states"]["GOAL1"],
        "review"
    );
    let review_updated_at = state.body["supervisor_agent"]["updated_at"]
        .as_str()
        .unwrap()
        .to_string();

    let override_attempt = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/supervisor-agent/session".to_string(),
        body: Some(json!({"provider": "smoke-ai"})),
    });
    assert_eq!(override_attempt.status, 400);
    assert!(
        override_attempt.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("configured agent_cli provider")
    );

    let first = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/supervisor-agent/session".to_string(),
        body: Some(json!({})),
    });
    let second = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/supervisor-agent/session".to_string(),
        body: Some(json!({})),
    });
    assert_eq!((first.status, second.status), (200, 200));
    assert_eq!(first.body["session_id"], second.body["session_id"]);
    assert_eq!(first.body["mode"], "supervisor");
    assert_eq!(first.body["provider"], "smoke-ai");

    write_goal_status("done");

    let restored = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/supervisor-agent".to_string(),
        body: None,
    });
    assert_eq!(
        restored.body["supervisor_agent"]["session_id"],
        first.body["session_id"]
    );
    assert_eq!(
        restored.body["supervisor_agent"]["goal_states"]["GOAL1"],
        "done"
    );
    assert_ne!(
        restored.body["supervisor_agent"]["updated_at"]
            .as_str()
            .unwrap(),
        review_updated_at
    );
    assert!(
        restored.body["supervisor_agent"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["message"] == "Goal GOAL1 changed from review to done.")
    );
    assert!(refine_dir.join("supervisor-agent.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_manages_refine_chat_sessions() {
    let temp_root = unique_temp_dir("http-chat");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        body: Some(json!({"goal_id": "GOAL1", "provider": "smoke-ai"})),
    });
    assert_eq!(started.status, 201);
    let session_id = started.body["session_id"].as_str().unwrap().to_string();
    assert_eq!(started.body["mode"], "goal");
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
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
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
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
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
fn web_server_submit_standalone_chat_creates_ready_merge_goal_and_preserves_worktree() {
    let temp_root = unique_temp_dir("http-chat-standalone-submit");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
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
            "prompt": "Standalone experiment is ready for the merge workflow.",
            "priority": "medium"
        })),
    });
    assert_eq!(submitted.status, 201, "{submitted:#?}");
    let goal_id = submitted.body["goal"]["id"].as_str().unwrap().to_string();
    assert_eq!(submitted.body["goal"]["status"], "ready-merge");
    assert_eq!(submitted.body["goal"]["branch_name"], branch);
    assert_eq!(submitted.body["goal"]["priority"], "medium");
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
    assert_eq!(session["worktree"]["submitted_goal_id"], goal_id);

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
    let legacy_refine_dir = app_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&legacy_refine_dir).unwrap();
    git(&app_root, &["init", "-q"]).unwrap();
    let refine_dir =
        crate::tools::host::project_layout::refine_dir_for_target_root(&app_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/project/status".to_string(),
        body: None,
    });
    assert_eq!(status.status, 200, "{:#}", status.body);
    assert_eq!(status.body["attached"], true);
    assert_eq!(status.body["target_root"], app_root.display().to_string());
    assert_eq!(status.body["apps"].as_array().unwrap().len(), 1);
    assert!(runtime_root.join("apps.json").exists());
    assert!(!temp_root.join("run/apps.json").exists());

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
    git(&third_app, &["init", "-q"]).unwrap();
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
    assert!(refine_dir.join("nodes.json").exists());
    assert!(!refine_dir.join("settings.json").exists());

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
    assert!(
        refine_dir_for_target_root(&destination)
            .unwrap()
            .join("refine.json")
            .exists()
    );
    assert!(!destination.join(".refine").exists());
    assert!(runtime_root.join("processes").exists());
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

    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        let created = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": format!("Instant runtime settings {id}")})),
        });
        assert_eq!(created.status, 201);
    }

    let updated = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "backlog_promote_after_seconds": "0"
        })),
    });
    assert_eq!(updated.status, 200);
    assert_eq!(updated.body["settings"]["parallel_run_cap"], "2");
    assert_eq!(
        updated.body["settings"]["backlog_promote_after_seconds"],
        "0"
    );

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["policy"]["global_limit"], 2);
    assert_eq!(state["policy"]["per_node_limit"], 2);
    assert_eq!(state["claims"].as_array().unwrap().len(), 2);

    let raised = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "parallel_run_cap": 3,
            "parallel_per_node_cap": 3
        })),
    });
    assert_eq!(raised.status, 200);
    assert_eq!(raised.body["settings"]["parallel_run_cap"], "3");

    let state = fs::read_to_string(runtime_root.join("workflow-automation-state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["policy"]["global_limit"], 3);
    assert_eq!(state["policy"]["per_node_limit"], 3);
    assert_eq!(state["claims"].as_array().unwrap().len(), 3);

    let goal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(goal.status, 200);
    assert_eq!(goal.body["goal"]["status"], "todo");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_requires_an_agent_for_legacy_project_state() {
    let temp_root = unique_temp_dir("http-project-migration");
    let runtime_root = temp_root.join("run/8080");
    let app_root = temp_root.join("legacy-app");
    let refine_dir = app_root.join(".refine");
    fs::create_dir_all(refine_dir.join("gaps/GA")).unwrap();
    fs::write(refine_dir.join("gaps/GA/gap.json"), "{}").unwrap();

    let mut server = server_with_projection();
    server.runtime_root = Some(runtime_root.clone());

    let blocked_attach = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/project/attach".to_string(),
        body: Some(json!({"path": app_root.display().to_string()})),
    });
    assert_eq!(blocked_attach.status, 409);
    assert!(
        blocked_attach.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("migration agent")
    );
    assert!(refine_dir.join("gaps/GA/gap.json").exists());
    assert!(!refine_dir.join("refine.json").exists());

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

    let blocked_switch = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/apps/switch".to_string(),
        body: Some(json!({"name": "second"})),
    });
    assert_eq!(blocked_switch.status, 409);
    assert!(
        blocked_switch.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("migration agent")
    );
    assert!(!second_refine_dir.join("refine.json").exists());

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
    assert!(runtime_root.join("apps.json").exists());
    assert!(!temp_root.join("run/apps.json").exists());

    let settings = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({"agent_cli": "smoke-ai"})),
    });
    assert_eq!(settings.status, 200);
    assert_eq!(settings.body["settings"]["agent_cli"], "smoke-ai");
    assert!(refine_dir.join("nodes.json").exists());
    assert!(!refine_dir.join("settings.json").exists());

    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"name": "Detached attach goal", "id": "GOAL1"})),
    });
    assert_eq!(created.status, 201);
    assert!(refine_dir.join("goals/GO/AL1/goal.json").exists());

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
    assert!(sse_body.contains("\"goal_count\":1"));

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
        r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest run"}}"#,
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
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Dashboard Goal"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL2", "name": "Finished Dashboard Goal"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL3", "name": "Cancelled Dashboard Goal"})),
    });
    let create_node = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes".to_string(),
        body: Some(json!({"id": "refine2"})),
    });
    assert_eq!(create_node.status, 200);
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/rounds".to_string(),
        body: Some(json!({"reporter": "Alice", "prompt": "Works"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL2/rounds".to_string(),
        body: Some(json!({
            "reporter": "Alice",
            "assignee": "Carol",
            "prompt": "Works"
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL3/rounds".to_string(),
        body: Some(json!({"reporter": "Bob", "prompt": "Works"})),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL2"],
            "update": {"status": "done"}
        })),
    });
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL3"],
            "update": {"status": "cancelled"}
        })),
    });
    let transfer = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/nodes/transfer-goals".to_string(),
        body: Some(json!({
            "target_node_id": "refine2",
            "selected_ids": ["GOAL3"],
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
            goal_id: Some("GOAL1".to_string()),
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
    assert!(
        generated.body["config"]["start_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run dev")
    );
    assert_eq!(generated.body["config"]["start_command"], "");
    assert!(
        generated.body["settings"]["target_app_build_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run build")
    );
    assert_eq!(
        generated.body["settings"]["target_app_test_command"],
        "npm test"
    );
    assert_eq!(
        generated.body["settings"]["target_app_test_commands"],
        r#"[{"command":"npm test","enabled":true}]"#
    );
    assert_eq!(generated.body["config"]["tcp_check_port"], "3000");
    assert!(!temp_root.join(".refine/manage-app.sh").exists());

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
    assert!(
        generated_operation.result["config"]["start_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run dev")
    );
    assert_eq!(generated_operation.result["config"]["start_command"], "");
    let settings = FileSettingsService::new(&refine_dir).load().unwrap();
    assert!(
        settings["target_app_start_instructions"]
            .as_str()
            .unwrap()
            .contains("npm run dev")
    );
    assert_eq!(settings["target_app_test_command"], "npm test");
    assert_eq!(
        settings["target_app_test_commands"],
        r#"[{"command":"npm test","enabled":true}]"#
    );

    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_build_instructions": "",
            "target_app_build_command": ""
        }))
        .unwrap();
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
    assert_eq!(cluster.body["enabled"], true);
    assert_eq!(cluster.body["nodes"][0]["id"], "default");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn web_server_target_app_status_is_passive_when_background_launches_are_stopped() {
    let temp_root = unique_temp_dir("http-target-status-passive");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&temp_root).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/settings".to_string(),
        body: Some(json!({
            "target_app_url": "http://127.0.0.1:3000",
            "target_app_status_command": "printf ok"
        })),
    });
    FileProcessSupervisor::new(&runtime_root)
        .set_background_processes_stopped(true)
        .unwrap();

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/target-app/status".to_string(),
        body: None,
    });
    assert_eq!(status.status, 200);
    assert_eq!(status.body["app_url"], "http://127.0.0.1:3000");
    assert_eq!(status.body["has_status_checks"], true);
    assert_eq!(status.body["state"], "unknown");

    let health = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/health".to_string(),
        body: None,
    });
    assert_eq!(health.status, 409);
    assert_eq!(
        health.body["error"]["message"],
        "background process launch is stopped"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

fn server_with_projection() -> InProcessWebServer {
    let mut goals = BTreeMap::new();
    goals.insert(
        "GOAL1".to_string(),
        GoalSummaryProjection {
            goal: GoalIndexProjection {
                id: "GOAL1".to_string(),
                name: "Projection route".to_string(),
                status: GoalStatus::Todo,
                priority: GoalPriority::Medium,
                reporter: Some("Buddy".to_string()),
                assignee: Some("Buddy".to_string()),
                round_count: 1,
                created: "created".to_string(),
                updated: "updated".to_string(),
                branch_name: None,
                node_id: Some("default".to_string()),
                feature_id: None,
                feature_order: None,
                json_path: "goals/01/GOAL1/goal.json".to_string(),
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
            goals,
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
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-b", "main"]).unwrap();
    git(repo, &["config", "user.email", "test@example.com"]).unwrap();
    git(repo, &["config", "user.name", "Test User"]).unwrap();
    fs::write(repo.join("app.txt"), "base\n").unwrap();
    git(repo, &["add", "app.txt"]).unwrap();
    git(repo, &["commit", "-m", "initial"]).unwrap();
    fs::create_dir_all(refine_dir_for_target_root(repo).unwrap()).unwrap();
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
    wait_for_http_request_metric_count(runtime_root, 1)
}

fn wait_for_http_request_metric_count(
    runtime_root: &Path,
    expected: usize,
) -> Vec<crate::tools::observability::metrics::PerformanceEvent> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let report = FileMetricsService::new(runtime_root)
            .report(PerformanceQuery {
                operation: Some("http.request".to_string()),
                ..PerformanceQuery::default()
            })
            .unwrap();
        if report.events.len() >= expected {
            return report.events;
        }
        if Instant::now() >= deadline {
            return report.events;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_project_sync_operation(
    runtime_root: &Path,
    response: &ApiResponse,
    expected: OperationState,
) -> OperationHandle {
    assert_eq!(response.status, 202, "{:#}", response.body);
    let operation_id = response.body["operation"]["id"]
        .as_str()
        .expect("project sync response should include an operation id");
    wait_for_operation_status(
        &FileOperationRegistry::new(runtime_root),
        operation_id,
        expected,
    )
}

fn wait_for_operation_status(
    registry: &FileOperationRegistry,
    operation_id: &str,
    expected: OperationState,
) -> OperationHandle {
    let deadline = Instant::now() + Duration::from_secs(15);
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
    let temp_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
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
