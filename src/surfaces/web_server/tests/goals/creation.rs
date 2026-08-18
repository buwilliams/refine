use super::*;

#[test]
fn web_server_creates_goal_from_new_goal_modal_payload() {
    let temp_root = unique_temp_dir("http-goal-create-modal");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    crate::infrastructure::process::supervisor::config::FileReporterService::new(&refine_dir)
        .create("Existing")
        .unwrap();

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

    let reporters = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/reporters".to_string(),
        body: None,
    });
    let reporter_names = reporters.body["reporters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|reporter| reporter["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(reporter_names, vec!["Alice", "Existing"]);

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
}

#[test]
fn warmed_goal_create_post_completes_under_fifty_milliseconds_at_current_scale() {
    const GOAL_COUNT: usize = 50;
    #[cfg(not(target_os = "macos"))]
    const MAX_REQUEST_TIME: Duration = Duration::from_millis(50);
    // Debug-mode durable filesystem writes are materially slower on APFS, while
    // the same warmed-cache contract still catches accidental full rebuilds.
    #[cfg(target_os = "macos")]
    const MAX_REQUEST_TIME: Duration = Duration::from_millis(500);

    let temp_root = unique_temp_dir("http-goal-create-performance");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let fixture_timestamp = Utc::now().to_rfc3339();
    for index in 0..GOAL_COUNT {
        let id = format!("GOAL{index:04}");
        let goal_path = refine_dir
            .join("goals")
            .join(&id[..2])
            .join(&id[2..])
            .join("goal.json");
        fs::create_dir_all(goal_path.parent().unwrap()).unwrap();
        fs::write(
            goal_path,
            serde_json::to_vec_pretty(&json!({
                "id": id,
                "name": format!("Performance fixture {index}"),
                "status": "backlog",
                "priority": "low",
                "reporter": "Performance",
                "assignee": "Performance",
                "branch_name": null,
                "feature_id": null,
                "feature_order": null,
                "node_id": "default",
                "created": fixture_timestamp,
                "updated": fixture_timestamp,
                "notes": [],
                "rounds": [{
                    "reporter": "Performance",
                    "assignee": "Performance",
                    "prompt": format!("Performance prompt {index}"),
                    "created": fixture_timestamp,
                    "updated": fixture_timestamp,
                    "guidance_decision": null,
                    "governance": null,
                    "quality": null,
                    "logs": []
                }]
            }))
            .unwrap(),
        )
        .unwrap();
    }
    // These fixtures bypass Goal authoring, so establish the durable invariant
    // that real Goal creation would have established before timing a warmed write.
    crate::infrastructure::process::supervisor::config::FileReporterService::new(&refine_dir)
        .create("Performance")
        .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);
    server.warm_current_projection_cache().unwrap();
    FileProjectProjectionStore::reset_rebuild_count(&refine_dir);

    let duplicate_started = Instant::now();
    let duplicate = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Performance",
            "prompt": format!("Performance prompt {}", GOAL_COUNT - 1)
        })),
    });
    let duplicate_elapsed = duplicate_started.elapsed();
    assert_eq!(duplicate.status, 409);
    assert_eq!(
        duplicate.body["error"]["duplicate"]["match"]["id"],
        format!("GOAL{:04}", GOAL_COUNT - 1)
    );
    assert_eq!(
        FileProjectProjectionStore::rebuild_count(&refine_dir),
        0,
        "a warmed duplicate decision must not rebuild the projection"
    );

    let create_started = Instant::now();
    let created = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Performance",
            "prompt": "A distinct warmed-cache Goal"
        })),
    });
    let create_elapsed = create_started.elapsed();
    assert_eq!(created.status, 201);
    assert_eq!(created.body["goal"]["round_count"], 1);
    assert_eq!(
        FileProjectProjectionStore::rebuild_count(&refine_dir),
        1,
        "a successful create must rebuild the complete projection exactly once"
    );
    let projection = server.current_projection().unwrap();
    assert_eq!(
        projection
            .goals
            .values()
            .filter(|goal| goal.goal.id.starts_with("GOAL"))
            .filter(|goal| goal.goal.status == GoalStatus::Backlog)
            .count(),
        GOAL_COUNT,
        "fresh performance fixtures must not turn the create benchmark into a bulk promotion test"
    );

    eprintln!(
        "warmed POST /api/goals timings at {GOAL_COUNT} Goals: late duplicate={duplicate_elapsed:?}, create={create_elapsed:?}"
    );
    assert!(
        duplicate_elapsed < MAX_REQUEST_TIME,
        "late duplicate POST took {duplicate_elapsed:?}, expected < {MAX_REQUEST_TIME:?}"
    );
    assert!(
        create_elapsed < MAX_REQUEST_TIME,
        "successful create POST took {create_elapsed:?}, expected < {MAX_REQUEST_TIME:?}"
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn warmed_goal_create_detects_an_external_latest_round_change() {
    let temp_root = unique_temp_dir("http-goal-create-external-change");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);
    server.warm_current_projection_cache().unwrap();

    let external = FileWorkItemService::new(&refine_dir);
    external
        .create_goal_summary("Externally created", Some("EXT1"))
        .unwrap();
    external
        .append_goal_round_summary("EXT1", "External daemon", "External duplicate prompt")
        .unwrap();

    let duplicate = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({
            "reporter": "Performance",
            "prompt": "External duplicate prompt"
        })),
    });

    assert_eq!(duplicate.status, 409);
    assert_eq!(duplicate.body["error"]["code"], "duplicate_goal");
    assert_eq!(duplicate.body["error"]["duplicate"]["match"]["id"], "EXT1");

    remove_temp_dir(&temp_root);
}

#[test]
fn concurrent_goal_create_requests_make_one_auditable_duplicate_decision() {
    let temp_root = unique_temp_dir("http-goal-create-concurrent");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root);
    server.warm_current_projection_cache().unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let server = server.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/api/goals".to_string(),
                body: Some(json!({
                    "reporter": "Concurrent daemon",
                    "prompt": "One coherent concurrent prompt"
                })),
            })
        }));
    }
    barrier.wait();
    let mut responses = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    responses.sort_by_key(|response| response.status);

    assert_eq!(responses[0].status, 201);
    assert_eq!(responses[1].status, 409);
    assert_eq!(responses[1].body["error"]["code"], "duplicate_goal");
    let projection = FileProjectProjectionStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    assert_eq!(projection.goals.len(), 1);
    assert_eq!(
        projection
            .goals
            .values()
            .next()
            .and_then(|goal| goal.latest_round_prompt.as_deref()),
        Some("One coherent concurrent prompt")
    );

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_submit_standalone_chat_creates_quality_goal_and_preserves_worktree() {
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

    let retired_submit = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/submit-ready-merge"),
        body: Some(json!({})),
    });
    assert_eq!(retired_submit.status, 404);

    let submitted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/chat/{session_id}/submit-quality"),
        body: Some(json!({
            "reporter": "QA",
            "prompt": "Standalone experiment is ready for the Governance workflow.",
            "priority": "medium"
        })),
    });
    assert_eq!(submitted.status, 201, "{submitted:#?}");
    let goal_id = submitted.body["goal"]["id"].as_str().unwrap().to_string();
    assert_eq!(submitted.body["goal"]["status"], "quality");
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

    remove_temp_dir(&temp_root);
}
