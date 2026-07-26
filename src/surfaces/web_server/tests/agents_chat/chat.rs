use super::*;

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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
}
