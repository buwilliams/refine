use super::*;

#[test]
fn durable_state_ignores_transient_lock_temp_and_copy_files() {
    let root = unique_temp_dir("transient-state");
    let sessions = root.join("chat/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(sessions.join("session.json"), "{}\n").unwrap();
    fs::write(sessions.join(".session.lock"), "").unwrap();
    fs::write(sessions.join("session.json.interrupted.tmp"), "partial\n").unwrap();
    fs::write(sessions.join(".refine-sync-123-0"), "partial\n").unwrap();
    fs::write(root.join("supervisor-agent.lock"), "").unwrap();

    let state = durable_state_map(&root).unwrap();

    assert_eq!(
        state.keys().cloned().collect::<Vec<_>>(),
        vec![PathBuf::from("chat/sessions/session.json")]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn durable_state_ignores_sharded_goal_log_sidecars() {
    let root = unique_temp_dir("goal-log-sidecars");
    // Goal records are sharded, so a log sidecar's leading component is `goals`
    // rather than `logs`. Excluding on the leading component alone published
    // every node's agent logs to refine/state.
    let goal = root.join("goals/GO/ALA");
    fs::create_dir_all(&goal).unwrap();
    fs::write(goal.join("goal.json"), "{\"id\":\"GOALA\"}\n").unwrap();
    fs::write(goal.join("logs.jsonl"), "{\"message\":\"agent\"}\n").unwrap();
    fs::write(goal.join("implementation-report.md"), "# Report\n").unwrap();
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::write(root.join("logs/daemon.jsonl"), "{}\n").unwrap();

    let state = durable_state_map(&root).unwrap();

    // The Goal record and its report stay durable; only the log sidecar is
    // node-local evidence.
    assert_eq!(
        state.keys().cloned().collect::<Vec<_>>(),
        vec![
            PathBuf::from("goals/GO/ALA/goal.json"),
            PathBuf::from("goals/GO/ALA/implementation-report.md"),
        ]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sync_retires_goal_logs_already_published_to_state() {
    let fixture = SyncFixture::new("retire-goal-logs");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();

    // Stand in for state published by a node predating the exclusion: a sharded
    // per-Goal log sidecar already committed to refine/state.
    let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
    let tracked_log = state_worktree.join(".refine/goals/GO/ALA/logs.jsonl");
    fs::create_dir_all(tracked_log.parent().unwrap()).unwrap();
    fs::write(&tracked_log, "{\"message\":\"agent output\"}\n").unwrap();
    git(&state_worktree, &["add", "-f", "--", ".refine"]);
    git(&state_worktree, &["commit", "-q", "-m", "publish goal logs"]);
    git(&state_worktree, &["push", "-q", "origin", "HEAD:refine/state"]);
    assert!(
        git_stdout(&state_worktree, &["ls-files", "--", ".refine"]).contains("logs.jsonl"),
        "fixture did not publish a tracked log sidecar"
    );

    // A live sidecar must stay node-local and never be published.
    let live_log = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GO/ALA/logs.jsonl");
    fs::create_dir_all(live_log.parent().unwrap()).unwrap();
    fs::write(&live_log, "{\"message\":\"local only\"}\n").unwrap();

    fixture.service(&fixture.a).sync().unwrap();

    let tracked = git_stdout(&state_worktree, &["ls-files", "--", ".refine"]);
    assert!(
        !tracked.contains("logs.jsonl"),
        "log sidecar stayed tracked on refine/state: {tracked}"
    );
    assert!(
        tracked.contains("goal.json"),
        "retiring logs must not drop Goal records: {tracked}"
    );
    assert!(
        live_log.exists(),
        "the live log sidecar must survive as node-local evidence"
    );
}
