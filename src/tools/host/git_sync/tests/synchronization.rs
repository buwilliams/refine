use super::*;

#[test]
fn sync_rebases_disjoint_state_when_nodes_race() {
    let fixture = SyncFixture::new("race");
    write_goal(&fixture.a, "GOALA");
    write_goal(&fixture.b, "GOALB");

    fixture.service(&fixture.a).sync().unwrap();
    let second = fixture.service(&fixture.b).sync().unwrap();
    assert!(
        second.committed && second.pulled && second.pushed,
        "{second:?}"
    );

    fixture.service(&fixture.a).sync().unwrap();
    let refine_dir = refine_dir_for_target_root(&fixture.a).unwrap();
    assert!(refine_dir.join("goals/GOALA/goal.json").exists());
    assert!(refine_dir.join("goals/GOALB/goal.json").exists());
}

#[test]
fn sync_recovers_completed_state_copy_interrupted_before_commit() {
    let fixture = SyncFixture::new("interrupted-copy-restart");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    let live_goal = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GOALA/goal.json");
    let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
    let state_goal = state_worktree.join(".refine/goals/GOALA/goal.json");
    fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"copied\"}\n").unwrap();
    copy_state_file(&live_goal, &state_goal).unwrap();
    assert_eq!(
        git_stdout(&state_worktree, &["status", "--short"]),
        "M .refine/goals/GOALA/goal.json"
    );

    write_goal(&fixture.b, "GOALB");
    fixture.service(&fixture.b).sync().unwrap();
    let remote_before_recovery = git_stdout(&fixture.a, &["ls-remote", "origin", REFINE_STATE_REF])
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"concurrent\"}\n").unwrap();

    let recovered = fixture.service(&fixture.a).sync().unwrap();

    assert!(
        recovered.committed && recovered.pulled && recovered.pushed,
        "{recovered:?}"
    );
    assert!(
        recovered
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("Recovered an interrupted Refine state copy")),
        "{recovered:?}"
    );
    assert_eq!(git_stdout(&state_worktree, &["status", "--short"]), "");
    assert_eq!(
        fs::read_to_string(&live_goal).unwrap(),
        "{\"id\":\"GOALA\",\"status\":\"concurrent\"}\n"
    );
    assert_eq!(
        git_stdout(
            &fixture.a,
            &["show", "origin/refine/state:.refine/goals/GOALA/goal.json",],
        ),
        "{\"id\":\"GOALA\",\"status\":\"concurrent\"}"
    );
    assert!(
        !git_stdout(
            &fixture.a,
            &["show", "origin/refine/state:.refine/goals/GOALB/goal.json",],
        )
        .is_empty()
    );
    git(
        &fixture.a,
        &[
            "merge-base",
            "--is-ancestor",
            &remote_before_recovery,
            "origin/refine/state",
        ],
    );
}

#[test]
fn sync_skips_noop_commits_and_summarizes_batches() {
    let fixture = SyncFixture::new("batch");
    write_goal(&fixture.a, "GOALA");
    write_goal(&fixture.a, "GOALB");

    let first = fixture.service(&fixture.a).sync().unwrap();
    assert!(first.committed && first.pushed, "{first:?}");
    let subject = git_stdout(&fixture.a, &["log", "-1", "--format=%s", "refine/state"]);
    assert_eq!(subject, "Sync Refine state: 2 goals");

    let second = fixture.service(&fixture.a).sync().unwrap();
    assert!(!second.committed && !second.pushed, "{second:?}");
    assert_eq!(
        git_stdout(&fixture.a, &["rev-list", "--count", "refine/state"]),
        "1"
    );
}

#[test]
fn sync_reports_same_record_multi_node_conflicts() {
    let fixture = SyncFixture::new("same-record-conflict");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    fs::write(
        refine_dir_for_target_root(&fixture.a)
            .unwrap()
            .join("goals/GOALA/goal.json"),
        "{\"id\":\"GOALA\",\"status\":\"review\"}\n",
    )
    .unwrap();
    fs::write(
        refine_dir_for_target_root(&fixture.b)
            .unwrap()
            .join("goals/GOALA/goal.json"),
        "{\"id\":\"GOALA\",\"status\":\"qa\"}\n",
    )
    .unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    let error = fixture.service(&fixture.b).sync().unwrap_err();
    assert!(
        error.to_string().contains("goals/GOALA/goal.json"),
        "{error}"
    );
}

#[test]
fn state_commit_summary_counts_sharded_records() {
    assert_eq!(
        state_commit_summary(
            "M  .refine/goals/GO/AL1/goal.json\nM  .refine/goals/GO/AL2/goal.json"
        ),
        "Sync Refine state: 2 goals"
    );
}

#[test]
fn sync_does_not_publish_transient_state_artifacts() {
    let fixture = SyncFixture::new("transient-artifacts");
    write_goal(&fixture.a, "GOALA");
    let refine_dir = refine_dir_for_target_root(&fixture.a).unwrap();
    let sessions = refine_dir.join("chat/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(sessions.join(".session.lock"), "").unwrap();
    fs::write(sessions.join("session.json.interrupted.tmp"), "partial\n").unwrap();
    fs::write(sessions.join(".refine-sync-123-0"), "partial\n").unwrap();

    let result = fixture.service(&fixture.a).sync().unwrap();

    assert!(result.committed && result.pushed, "{result:?}");
    assert_eq!(
        git_stdout(
            &fixture.a,
            &["ls-tree", "-r", "--name-only", REFINE_STATE_BRANCH]
        ),
        ".refine/goals/GOALA/goal.json"
    );
}

#[test]
fn sync_removes_transient_artifacts_already_on_state_branch() {
    let fixture = SyncFixture::new("stale-transient-artifacts");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
    let stale = state_worktree.join(".refine/chat/sessions/.session.lock");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "stale\n").unwrap();
    git(&state_worktree, &["add", "-f", ".refine"]);
    git(
        &state_worktree,
        &["commit", "-q", "-m", "publish stale lock"],
    );
    git(
        &state_worktree,
        &["push", "-q", "origin", REFINE_STATE_BRANCH],
    );

    let result = fixture.service(&fixture.a).sync().unwrap();

    assert!(result.committed && result.pushed, "{result:?}");
    assert!(!stale.exists());
    assert_eq!(
        git_stdout(
            &fixture.a,
            &["ls-tree", "-r", "--name-only", REFINE_STATE_BRANCH]
        ),
        ".refine/goals/GOALA/goal.json"
    );
}

#[test]
fn failed_state_copy_removes_its_partial_temp_file() {
    let root = unique_temp_dir("failed-copy-cleanup");
    let source = root.join("source-directory");
    let destination = root.join("destination/state.json");
    fs::create_dir_all(&source).unwrap();

    assert!(copy_state_file(&source, &destination).is_err());
    assert_eq!(
        fs::read_dir(destination.parent().unwrap()).unwrap().count(),
        0
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn copy_back_preserves_mutations_that_arrive_during_sync() {
    let root = unique_temp_dir("copy-back-race");
    let live = root.join("live");
    let state = root.join("state");
    fs::create_dir_all(&live).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(live.join("goal.json"), "before\n").unwrap();
    let original = durable_state_map(&live).unwrap();
    fs::write(live.join("goal.json"), "concurrent\n").unwrap();
    fs::write(state.join("goal.json"), "remote\n").unwrap();
    fs::write(state.join("remote.json"), "remote-only\n").unwrap();

    assert!(merge_state_into_live(&state, &live, &original).unwrap());
    assert_eq!(
        fs::read_to_string(live.join("goal.json")).unwrap(),
        "concurrent\n"
    );
    assert_eq!(
        fs::read_to_string(live.join("remote.json")).unwrap(),
        "remote-only\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sync_does_not_touch_uncommitted_target_app_changes() {
    let fixture = SyncFixture::new("dirty");
    fs::write(fixture.a.join("app.txt"), "dirty\n").unwrap();
    write_goal(&fixture.a, "GOALA");
    let head = git_stdout(&fixture.a, &["rev-parse", "HEAD"]);

    let result = fixture.service(&fixture.a).sync().unwrap();
    assert!(result.attempted && result.committed && result.pushed);
    assert!(
        refine_dir_for_target_root(&fixture.a)
            .unwrap()
            .join("goals/GOALA/goal.json")
            .exists()
    );
    assert_eq!(git_stdout(&fixture.a, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(fixture.a.join("app.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn state_demand_fetches_only_state_while_project_pulse_fetches_all_branches() {
    let fixture = SyncFixture::new("fetch-scopes");
    let original_remote_main = git_stdout(&fixture.a, &["rev-parse", "origin/main"]);
    fs::write(fixture.b.join("app.txt"), "human change\n").unwrap();
    git(&fixture.b, &["add", "app.txt"]);
    git(&fixture.b, &["commit", "-q", "-m", "human change"]);
    git(&fixture.b, &["push", "-q", "origin", "main"]);
    let human_commit = git_stdout(&fixture.b, &["rev-parse", "HEAD"]);
    assert_ne!(human_commit, original_remote_main);

    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).try_sync_state().unwrap();
    assert_eq!(
        git_stdout(&fixture.a, &["rev-parse", "origin/main"]),
        original_remote_main
    );

    fixture.service(&fixture.a).try_sync().unwrap();
    assert_eq!(
        git_stdout(&fixture.a, &["rev-parse", "origin/main"]),
        human_commit
    );
    assert_eq!(
        git_stdout(&fixture.a, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        git_stdout(&fixture.a, &["rev-parse", "HEAD"]),
        original_remote_main
    );
}

#[test]
fn sync_requires_legacy_state_to_be_removed_from_application_branch() {
    let fixture = SyncFixture::new("legacy-tracked");
    let legacy_goal = fixture.a.join(".refine/goals/GOALA");
    fs::create_dir_all(&legacy_goal).unwrap();
    fs::write(legacy_goal.join("goal.json"), "{\"id\":\"GOALA\"}\n").unwrap();
    git(&fixture.a, &["add", ".refine"]);
    git(&fixture.a, &["commit", "-m", "legacy Refine state"]);
    fs::write(
        fixture.a.join(".refine/goals/GOALA/goal.json"),
        "{\"id\":\"GOALA\",\"status\":\"review\"}\n",
    )
    .unwrap();

    let error = fixture.service(&fixture.a).sync().unwrap_err();
    assert!(error.to_string().contains("still tracks legacy .refine"));
    assert!(!fixture.a.join(".refine").exists());

    git(&fixture.a, &["add", "-u", "--", ".refine"]);
    git(&fixture.a, &["commit", "-m", "Remove legacy Refine state"]);
    let app_head = git_stdout(&fixture.a, &["rev-parse", "HEAD"]);
    let result = fixture.service(&fixture.a).sync().unwrap();
    assert!(result.committed && result.pushed, "{result:?}");
    assert_eq!(git_stdout(&fixture.a, &["rev-parse", "HEAD"]), app_head);
    assert_eq!(git_stdout(&fixture.a, &["status", "--porcelain"]), "");
    assert!(!fixture.a.join(".refine").exists());
    assert!(
        git_stdout(
            &fixture.a,
            &["show", "refine/state:.refine/goals/GOALA/goal.json"]
        )
        .contains("review")
    );
}
