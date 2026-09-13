use super::*;

#[test]
fn durable_state_ignores_transient_lock_temp_and_copy_files() {
    let root = unique_temp_dir("transient-state");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("refine.json"), "{}\n").unwrap();
    fs::write(root.join(".refine.json.lock"), "").unwrap();
    fs::write(root.join("refine.json.interrupted.tmp"), "partial\n").unwrap();
    fs::write(root.join(".refine-sync-123-0"), "partial\n").unwrap();
    fs::write(root.join("supervisor-agent.lock"), "").unwrap();
    // Chat sessions embed their whole transcript and are rewritten per output
    // line; they are node-local runtime evidence, not synchronized state.
    let sessions = root.join("chat/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(sessions.join("session.json"), "{}\n").unwrap();

    let state = durable_state_map(&root).unwrap();

    assert_eq!(
        state.keys().cloned().collect::<Vec<_>>(),
        vec![PathBuf::from("refine.json")]
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
    git(
        &state_worktree,
        &["commit", "-q", "-m", "publish goal logs"],
    );
    git(
        &state_worktree,
        &["push", "-q", "origin", "HEAD:refine/state"],
    );
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

#[test]
fn durable_state_ignores_active_node_selection() {
    let root = unique_temp_dir("active-node-selection");
    fs::create_dir_all(root.join("runtime")).unwrap();
    fs::write(root.join("refine.json"), "{}\n").unwrap();
    // Node identity is machine-local: one synced selection pins the whole
    // fleet to a single node id and the ownership guards then refuse every
    // other node's goals. Neither the canonical runtime-local file nor a
    // legacy root-level copy may reach durable state.
    fs::write(root.join("runtime/active-node.json"), "{}\n").unwrap();
    fs::write(root.join("active-node.json"), "{}\n").unwrap();

    let state = durable_state_map(&root).unwrap();

    assert_eq!(
        state.keys().cloned().collect::<Vec<_>>(),
        vec![PathBuf::from("refine.json")]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sync_retires_active_node_selection_already_published_to_state() {
    let fixture = SyncFixture::new("retire-active-node");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();

    // Stand in for a fleet poisoned before the exclusion: one machine's
    // active-node selection committed to refine/state, which every syncing
    // node then read as its own identity.
    let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
    let tracked_selection = state_worktree.join(".refine/active-node.json");
    fs::write(
        &tracked_selection,
        "{\"active_node_id\":\"BO2LNXIPSAPP01\"}\n",
    )
    .unwrap();
    git(&state_worktree, &["add", "-f", "--", ".refine"]);
    git(
        &state_worktree,
        &["commit", "-q", "-m", "publish active node"],
    );
    git(
        &state_worktree,
        &["push", "-q", "origin", "HEAD:refine/state"],
    );
    assert!(
        git_stdout(&state_worktree, &["ls-files", "--", ".refine"]).contains("active-node.json"),
        "fixture did not publish a tracked active-node selection"
    );

    fixture.service(&fixture.a).sync().unwrap();

    let tracked = git_stdout(&state_worktree, &["ls-files", "--", ".refine"]);
    assert!(
        !tracked.contains("active-node.json"),
        "active-node selection stayed tracked on refine/state: {tracked}"
    );
    assert!(
        tracked.contains("goal.json"),
        "retiring the selection must not drop Goal records: {tracked}"
    );
}

// Synchronization compares content hashes across worktrees, so a memo that
// returned a stale hash would make a changed record look untouched and let one
// node's edit be silently dropped. Skipping re-reads is only safe if every real
// change still moves the hash.
#[test]
fn reusing_content_hashes_never_masks_a_change() {
    let root = unique_temp_dir("state-hash-memo");
    let record = root.join("goals/GO/AL1");
    fs::create_dir_all(&record).unwrap();
    let path = record.join("goal.json");
    fs::write(&path, "{\"id\":\"GOAL1\",\"status\":\"todo\"}\n").unwrap();

    let first = durable_state_map(&root).unwrap();
    // A second scan with nothing touched must agree with the first; this is the
    // path that reuses the cached hash.
    let unchanged = durable_state_map(&root).unwrap();
    assert_eq!(first, unchanged, "an untouched record changed identity");

    // Same length, different content: the case a size check alone would miss.
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&path, "{\"id\":\"GOAL1\",\"status\":\"done\"}\n").unwrap();
    let edited = durable_state_map(&root).unwrap();
    assert_ne!(
        first[&PathBuf::from("goals/GO/AL1/goal.json")],
        edited[&PathBuf::from("goals/GO/AL1/goal.json")],
        "an edited record must not reuse its previous hash"
    );

    // Restoring the original content restores the original hash: identity
    // tracks content, not the number of times a file was written.
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&path, "{\"id\":\"GOAL1\",\"status\":\"todo\"}\n").unwrap();
    let restored = durable_state_map(&root).unwrap();
    assert_eq!(first, restored);

    fs::remove_dir_all(root).unwrap();
}

// Local mutations keep the scheduler's view current as they write, but
// synchronization copies records straight into the live store without going
// through that path. Work another Node published would otherwise stay invisible
// to scheduling until something unrelated forced a reconstruction.
#[test]
fn synchronized_goals_reach_the_scheduler_index() {
    let fixture = SyncFixture::new("sync-active-index");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    let b_refine = refine_dir_for_target_root(&fixture.b).unwrap();
    let pulled = ActiveGoalIndex::load_or_rebuild(&b_refine).unwrap();
    assert_eq!(
        pulled
            .goals()
            .map(|goal| goal.id.clone())
            .collect::<Vec<_>>(),
        vec!["GOALA"],
        "a Goal pulled from another node must be schedulable"
    );

    // And a Goal that reaches a terminal status elsewhere must leave this node's
    // index when that status arrives.
    let a_goal = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GOALA/goal.json");
    fs::write(&a_goal, "{\"id\":\"GOALA\",\"status\":\"done\"}\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    assert!(
        ActiveGoalIndex::load_or_rebuild(&b_refine)
            .unwrap()
            .is_empty(),
        "a Goal completed on another node must leave the scheduler index"
    );
}

#[test]
fn interrupted_synchronized_replace_and_delete_reconcile_without_revision_assumptions() {
    let root = unique_temp_dir("sync-index-interruption");
    let relative = PathBuf::from("goals/GO/ALA/goal.json");
    let path = root.join(&relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        br#"{"id":"GOALA","status":"done","workflow_revision":20}"#,
    )
    .unwrap();
    assert!(ActiveGoalIndex::load_or_rebuild(&root).unwrap().is_empty());

    // Synchronization can select bytes with a lower local revision. Compare
    // the intended source bytes, so a reader before replacement cannot consume
    // the marker merely because the old revision is numerically greater.
    let selected = br#"{"id":"GOALA","status":"todo","workflow_revision":4}"#;
    ActiveGoalIndex::prepare_goal_write(&root, &path, Some(selected)).unwrap();
    assert!(ActiveGoalIndex::load_or_rebuild(&root).unwrap().is_empty());
    replace_file_durably(&path, selected).unwrap();
    // Simulate the synchronizer exiting before reconcile_hydrated_index.
    assert_eq!(ActiveGoalIndex::load_or_rebuild(&root).unwrap().len(), 1);

    ActiveGoalIndex::prepare_goal_write(&root, &path, None).unwrap();
    assert_eq!(ActiveGoalIndex::load_or_rebuild(&root).unwrap().len(), 1);
    fs::remove_file(&path).unwrap();
    assert!(ActiveGoalIndex::load_or_rebuild(&root).unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.join("runtime/active-goals-pending"))
            .unwrap()
            .count(),
        0
    );
    fs::remove_dir_all(root).unwrap();
}
