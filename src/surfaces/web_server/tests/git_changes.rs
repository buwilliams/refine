use super::*;

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
    assert_eq!(rebuilt.status, 202);
    let operation_id = rebuilt.body["operation"]["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let rebuilt_result = loop {
        let operation = FileOperationRegistry::new(&runtime_root)
            .status(operation_id)
            .unwrap();
        if operation.state == OperationState::Succeeded {
            break operation.result;
        }
        assert!(
            Instant::now() < deadline,
            "background cache rebuild did not finish: {operation:?}"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(rebuilt_result["goals"], 1);
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
    let cached = FileProjectProjectionStore::new(&refine_dir)
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

    remove_temp_dir(&temp_root);
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
    assert_eq!(undo.status, 202);
    let undo_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        undo.body["operation"]["id"].as_str().unwrap(),
        OperationState::Succeeded,
    );
    assert_eq!(undo_operation.result["ok"], true);
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

    remove_temp_dir(&temp_root);
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
    assert_eq!(reset.status, 202);
    let reset_operation = wait_for_operation_status(
        &FileOperationRegistry::new(&runtime_root),
        reset.body["operation"]["id"].as_str().unwrap(),
        OperationState::Succeeded,
    );
    assert_eq!(reset_operation.result["ok"], true);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
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

    remove_temp_dir(&temp_root);
}
