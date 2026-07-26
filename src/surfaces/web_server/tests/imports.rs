use super::*;

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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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
    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
}
