use super::*;

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

    remove_temp_dir(&temp_root);
}

#[test]
fn web_api_stops_managed_and_synthetic_agents_through_shared_control() {
    let temp_root = unique_temp_dir("http-stop-goal-agent");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Stop selected agent", Some("GOAL-STOP-AGENT"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL-STOP-AGENT", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-STOP-AGENT", GoalStatus::InProgress)
        .unwrap();

    let agent_supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = agent_supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
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
            metadata: serde_json::Map::from_iter([
                ("kind".to_string(), json!("interactive_session")),
                ("provider".to_string(), json!("smoke-ai")),
                ("profile".to_string(), json!("goal")),
                ("session_id".to_string(), json!("goal-agent-session")),
                ("goal_id".to_string(), json!("GOAL-STOP-AGENT")),
            ]),
        })
        .unwrap();
    let pid = process.pid.unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200, "{}", listed.body);
    let listed_process = listed.body["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == process.id)
        .unwrap();
    assert_eq!(listed_process["kind"], "interactive_session");
    assert_eq!(listed_process["management_actions"], json!(["stop_agent"]));

    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/processes/{}/stop", process.id),
        body: Some(json!({"signal": "terminate"})),
    });
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.body["stopped"], true);
    assert_eq!(stopped.body["process"]["id"], process.id);
    assert_eq!(stopped.body["termination"]["confirmed_exit"], true);
    assert_eq!(stopped.body["goal"]["id"], "GOAL-STOP-AGENT");
    assert_eq!(stopped.body["goal"]["status"], "failed");
    assert_eq!(stopped.body["goal_failed"], true);
    assert_eq!(stopped.body["goal_requeued"], false);
    assert_eq!(stopped.body["worktrees_retained"], true);
    assert!(!managed_pid_is_alive(pid).unwrap());
    assert!(agent_supervisor.inspect(&process.id).is_err());

    work_items
        .create_goal_summary("Stop Goal chat", Some("GOAL-STOP-CHAT"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL-STOP-CHAT", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-STOP-CHAT", GoalStatus::InProgress)
        .unwrap();
    let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
    let session = chat
        .start_with_options(
            ChatAttachment::Goal("GOAL-STOP-CHAT".to_string()),
            Some("smoke-ai"),
            Some("goal"),
        )
        .unwrap();
    let stopped_chat = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/processes/chat-session-{}/stop", session.id),
        body: None,
    });
    assert_eq!(stopped_chat.status, 200, "{}", stopped_chat.body);
    assert_eq!(stopped_chat.body["process"]["status"], "stopped");
    assert_eq!(stopped_chat.body["termination"]["confirmed_exit"], true);
    assert_eq!(stopped_chat.body["termination"]["already_idle"], true);
    assert_eq!(stopped_chat.body["goal"]["id"], "GOAL-STOP-CHAT");
    assert_eq!(stopped_chat.body["goal"]["status"], "failed");
    assert_eq!(stopped_chat.body["goal_failed"], true);
    assert_eq!(stopped_chat.body["goal_requeued"], false);
    assert_eq!(stopped_chat.body["worktrees_retained"], true);
    assert!(
        chat.list_sessions()
            .unwrap()
            .iter()
            .find(|listed| listed.id == session.id)
            .unwrap()
            .closed
    );

    work_items
        .create_goal_summary("Stop through MCP", Some("GOAL-STOP-MCP"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL-STOP-MCP", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-STOP-MCP", GoalStatus::InProgress)
        .unwrap();
    let mcp_process = agent_supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
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
            metadata: serde_json::Map::from_iter([("goal_id".to_string(), json!("GOAL-STOP-MCP"))]),
        })
        .unwrap();
    let through_mcp = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/mcp".to_string(),
        body: Some(json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/call",
            "params": {
                "name": "refine_stop_process",
                "arguments": {"process_id": mcp_process.id}
            }
        })),
    });
    assert_eq!(through_mcp.status, 200, "{}", through_mcp.body);
    assert_eq!(through_mcp.body["result"]["isError"], false);
    assert_eq!(
        through_mcp.body["result"]["structuredContent"]["termination"]["confirmed_exit"],
        true
    );
    assert_eq!(
        through_mcp.body["result"]["structuredContent"]["goal"]["status"],
        "failed"
    );

    remove_temp_dir(&temp_root);
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

    let generic = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/agents/configured-generic-agent/diagnostics".to_string(),
        body: None,
    });
    assert_eq!(generic.status, 200);
    assert_eq!(generic.body["provider"], "configured-generic-agent");
    assert!(
        generic.body["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry
                .as_str()
                .unwrap_or("")
                .contains("configured-generic-agent CLI not found"))
    );

    let recheck = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/settings/recheck-auth".to_string(),
        body: None,
    });
    assert_eq!(recheck.status, 200);
    assert!(recheck.body["message"].as_str().unwrap().contains("CLI"));
}

#[test]
fn general_agent_prompt_routes_repository_changes_through_refine_workflow() {
    let prompt = crate::surfaces::web_server::work_routes::terminal_profile_prompt(
        &server_with_projection(),
        "agent",
        None,
        None,
        None,
    )
    .unwrap();

    assert!(prompt.contains("general-purpose native Agent"));
    assert!(prompt.contains("Treat Refine as the execution path for repository changes"));
    assert!(prompt.contains("inspect source, runtime state, logs, Git history"));
    assert!(prompt.contains("answer conversational questions"));
    assert!(prompt.contains("do not modify the repository ad hoc in this session"));
    assert!(prompt.contains("Autonomously translate the desired outcome"));
    assert!(prompt.contains("complete Refine Goal"));
    assert!(prompt.contains("actionable Round"));
    assert!(prompt.contains("make the Goal eligible for workflow execution"));
    assert!(prompt.contains("do not require the user to recite lifecycle commands"));
    assert!(prompt.contains("preserve that attempt"));
    assert!(prompt.contains("append a new Round"));
    assert!(prompt.contains("return the Goal to an eligible workflow state"));
    assert!(prompt.contains("Honor Refine's confirmation and audit boundaries"));
    assert!(prompt.contains("never directly edit durable Goal state"));
    assert!(prompt.contains("conceal failures"));
    assert!(prompt.contains("approve or merge on the user's behalf"));
    assert!(prompt.contains("destructively discard retained work"));
    assert!(prompt.contains("begin ongoing supervision unless the user requests it"));
    assert!(prompt.contains("Active Refine executable:"));
    assert!(prompt.contains("Resolved Refine source checkout:"));
    assert!(prompt.contains("checkout-local `./r`"));
    assert!(!prompt.contains("monitor the targeted app"));

    for profile in ["plan", "goal", "standalone"] {
        let other_prompt = crate::surfaces::web_server::work_routes::terminal_profile_prompt(
            &server_with_projection(),
            profile,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(
            !other_prompt.contains("Treat Refine as the execution path for repository changes"),
            "{profile} must retain its existing profile contract"
        );
    }
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

    remove_temp_dir(&temp_root);
}
