use super::*;

#[test]
fn web_server_appends_and_edits_latest_round() {
    let temp_root = unique_temp_dir("http-rounds");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    crate::process::supervisor::config::FileReporterService::new(&refine_dir)
        .create("Existing")
        .unwrap();
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
    let reporter_names = reporters.body["reporters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|reporter| reporter["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(reporter_names, vec!["Existing", "Reporter", "Reviewer"]);
    assert!(refine_dir.join("reporters.json").exists());

    remove_temp_dir(&temp_root);
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
    assert!(refine_dir.join("runtime/goals/GO/AL1/logs.jsonl").exists());

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
    let planning_write = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/goals/GOAL1/rounds/latest/evaluation".to_string(),
        body: Some(json!({
            "implementation_plan": {"state": "completed", "phase": "implement"}
        })),
    });
    assert_eq!(planning_write.status, 400, "{}", planning_write.body);
    assert!(
        planning_write.body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Workflow-owned evidence")),
        "{}",
        planning_write.body
    );

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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
