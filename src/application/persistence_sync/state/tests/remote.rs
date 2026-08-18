use super::*;

#[test]
fn sync_commits_pushes_and_pulls_refine_state() {
    let fixture = SyncFixture::new("round-trip");
    write_goal(&fixture.a, "GOALA");

    let pushed = fixture.service(&fixture.a).sync().unwrap();
    assert!(pushed.ok && pushed.committed && pushed.pushed, "{pushed:?}");

    let pulled = fixture.service(&fixture.b).sync().unwrap();
    assert!(pulled.ok && pulled.pulled && !pulled.pushed, "{pulled:?}");
    assert!(
        refine_dir_for_target_root(&fixture.b)
            .unwrap()
            .join("goals/GOALA/goal.json")
            .exists()
    );
    assert!(!fixture.a.join(".refine").exists());
    assert!(!fixture.b.join(".refine").exists());
    let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
    assert_eq!(state_worktree, fixture.a.join(".git/refine-state-worktree"));
    assert!(
        state_worktree
            .join(".refine/goals/GOALA/goal.json")
            .exists()
    );
    assert_eq!(
        git_stdout(&fixture.a, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        git_stdout(&fixture.b, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        git_stdout(
            &fixture.a,
            &["ls-tree", "-r", "--name-only", "refine/state"]
        ),
        ".refine/goals/GOALA/goal.json"
    );
}

#[test]
fn push_race_retry_remerges_and_reports_contested_records_with_push_retry_phase() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = SyncFixture::new("push-retry-conflict");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    let goal_a = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GOALA/goal.json");
    let goal_b = refine_dir_for_target_root(&fixture.b)
        .unwrap()
        .join("goals/GOALA/goal.json");
    fs::write(&goal_a, "{\"id\":\"GOALA\",\"status\":\"review\"}\n").unwrap();
    fs::write(&goal_b, "{\"id\":\"GOALA\",\"status\":\"qa\"}\n").unwrap();

    let remote = fixture.root.join("remote.git");
    let arrivals = fixture.root.join("push-arrivals");
    fs::create_dir_all(&arrivals).unwrap();
    let hook = remote.join("hooks/pre-receive");
    fs::write(
            &hook,
            format!(
                "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state; then\n    touch \"{}/$$\"\n    while test \"$(find \"{}\" -type f | wc -l)\" -lt 2; do sleep 0.01; done\n  fi\ndone\n",
                arrivals.display(),
                arrivals.display()
            ),
        )
        .unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let left_service = fixture.service(&fixture.a);
    let right_service = fixture.service(&fixture.b);
    let (left, right) = thread::scope(|scope| {
        let left_barrier = Arc::clone(&barrier);
        let left = scope.spawn(move || {
            left_barrier.wait();
            left_service.sync()
        });
        let right_barrier = Arc::clone(&barrier);
        let right = scope.spawn(move || {
            right_barrier.wait();
            right_service.sync()
        });
        (left.join().unwrap(), right.join().unwrap())
    });

    let errors = [left.as_ref().err(), right.as_ref().err()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "left={left:?}, right={right:?}");
    assert!(
        errors[0].to_string().contains(
            "needs a decision: this node and another changed the same record(s) (1 contested"
        ),
        "{}",
        errors[0]
    );
    let failed_root = if left.is_err() {
        &fixture.a
    } else {
        &fixture.b
    };
    let report = latest_state_sync_conflict_report(&failed_root.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(report.phase, StateSyncConflictPhase::PushRetry);
    assert_eq!(report.unresolved_paths, vec!["goals/GOALA/goal.json"]);
    assert!(left.is_ok() || right.is_ok());
    assert_eq!(
        fs::read_to_string(&goal_a).unwrap(),
        "{\"id\":\"GOALA\",\"status\":\"review\"}\n"
    );
    assert_eq!(
        fs::read_to_string(&goal_b).unwrap(),
        "{\"id\":\"GOALA\",\"status\":\"qa\"}\n"
    );
}

#[test]
fn push_race_retry_merges_disjoint_records_from_the_fresh_remote_head() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = SyncFixture::new("push-retry-merge");
    write_nodes(
        &fixture.a,
        &[
            ("node-a", "2026-08-17T08:00:00Z", "unknown"),
            ("node-b", "2026-08-17T08:00:00Z", "unknown"),
        ],
    );
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    write_nodes(
        &fixture.a,
        &[
            ("node-a", "2026-08-17T08:01:00Z", "healthy"),
            ("node-b", "2026-08-17T08:00:00Z", "unknown"),
        ],
    );
    write_nodes(
        &fixture.b,
        &[
            ("node-a", "2026-08-17T08:00:00Z", "unknown"),
            ("node-b", "2026-08-17T08:02:00Z", "healthy"),
        ],
    );

    let remote = fixture.root.join("remote.git");
    let arrivals = fixture.root.join("node-push-arrivals");
    fs::create_dir_all(&arrivals).unwrap();
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state; then\n    touch \"{}/$$\"\n    while test \"$(find \"{}\" -type f | wc -l)\" -lt 2; do sleep 0.01; done\n  fi\ndone\n",
            arrivals.display(),
            arrivals.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let left_service = fixture.service(&fixture.a);
    let right_service = fixture.service(&fixture.b);
    let (left, right) = thread::scope(|scope| {
        let left_barrier = Arc::clone(&barrier);
        let left = scope.spawn(move || {
            left_barrier.wait();
            left_service.sync()
        });
        let right_barrier = Arc::clone(&barrier);
        let right = scope.spawn(move || {
            right_barrier.wait();
            right_service.sync()
        });
        (left.join().unwrap(), right.join().unwrap())
    });

    let left = left.unwrap();
    let right = right.unwrap();
    assert!(
        left.pushed && right.pushed,
        "left={left:?}, right={right:?}"
    );
    git(&fixture.a, &["fetch", "-q", "origin", REFINE_STATE_BRANCH]);
    let remote_nodes: crate::model::node::NodeRegistry = serde_json::from_str(&git_stdout(
        &fixture.a,
        &["show", "origin/refine/state:.refine/nodes.json"],
    ))
    .unwrap();
    assert_eq!(remote_nodes.nodes[0].updated_at, "2026-08-17T08:01:00Z");
    assert_eq!(remote_nodes.nodes[1].updated_at, "2026-08-17T08:02:00Z");
}

#[test]
fn first_sync_hydrates_remote_state_over_local_bootstrap_metadata() {
    let fixture = SyncFixture::new("remote-first-bootstrap");
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    fs::create_dir_all(refine_a.join("quality")).unwrap();
    fs::create_dir_all(refine_b.join("quality")).unwrap();
    write_goal(&fixture.a, "GOALA");
    fs::write(
        refine_a.join("refine.json"),
        "{\"schema_version\":2,\"source\":\"remote\"}\n",
    )
    .unwrap();
    fs::write(
        refine_a.join("quality/settings.json"),
        "{\"instructions\":\"remote\"}\n",
    )
    .unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    fs::write(
        refine_b.join("refine.json"),
        "{\"schema_version\":2,\"source\":\"bootstrap\"}\n",
    )
    .unwrap();
    fs::write(
        refine_b.join("quality/settings.json"),
        "{\"instructions\":\"bootstrap\"}\n",
    )
    .unwrap();

    let hydrated = fixture.service(&fixture.b).sync().unwrap();

    assert!(!hydrated.committed, "{hydrated:?}");
    assert!(refine_b.join("goals/GOALA/goal.json").exists());
    assert_eq!(
        fs::read_to_string(refine_b.join("refine.json")).unwrap(),
        "{\"schema_version\":2,\"source\":\"remote\"}\n"
    );
    assert_eq!(
        fs::read_to_string(refine_b.join("quality/settings.json")).unwrap(),
        "{\"instructions\":\"remote\"}\n"
    );
    assert!(
        hydrated
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("bootstrap metadata")),
        "{hydrated:?}"
    );
}

#[test]
fn joining_with_unpublished_records_publishes_them_and_keeps_remote_records() {
    let fixture = SyncFixture::new("join-disjoint");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();

    // Node B has real (non-bootstrap) live records that were never published
    // anywhere, but nothing overlaps with the remote branch.
    write_goal(&fixture.b, "GOALB");

    let joined = fixture.service(&fixture.b).sync().unwrap();

    assert!(joined.committed && joined.pushed, "{joined:?}");
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert!(refine_b.join("goals/GOALA/goal.json").exists());
    assert!(refine_b.join("goals/GOALB/goal.json").exists());
    assert!(
        !git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/goals/GOALA/goal.json"],
        )
        .is_empty()
    );
    assert!(
        !git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/goals/GOALB/goal.json"],
        )
        .is_empty()
    );
}

#[test]
fn joining_with_contested_records_fails_closed_without_touching_either_side() {
    let fixture = SyncFixture::new("join-contested");
    write_goal(&fixture.a, "GOALA");
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    fs::create_dir_all(&refine_b).unwrap();
    fs::write(refine_a.join("shared.json"), "{\"node\":\"a\"}\n").unwrap();
    fs::write(refine_b.join("shared.json"), "{\"node\":\"b\"}\n").unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    fixture.service(&fixture.a).sync().unwrap_err();

    // A first contact has no merge base to arbitrate a contested record.
    assert!(refine_a.join("shared.json").exists());
    assert_eq!(
        fs::read_to_string(refine_a.join("shared.json")).unwrap(),
        "{\"node\":\"a\"}\n"
    );
    let report = latest_state_sync_conflict_report(&fixture.a.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(report.unresolved_paths, vec!["shared.json"]);
    // The local branch stays absent so the join re-derives on every rerun.
    assert!(
        !git_stdout(&fixture.a, &["branch", "--list", "refine/state"]).contains("refine/state")
    );
    assert!(
        !git_stdout(
            &fixture.a,
            &["show", "origin/refine/state:.refine/shared.json"],
        )
        .is_empty()
    );
}

#[test]
fn sync_uses_the_configured_git_remote() {
    let fixture = SyncFixture::new("configured-remote");
    git(&fixture.a, &["remote", "rename", "origin", "upstream"]);
    let refine_dir = refine_dir_for_target_root(&fixture.a).unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"git_remote": "upstream"}))
        .unwrap();
    write_goal(&fixture.a, "GOALA");

    let result = fixture.service(&fixture.a).sync().unwrap();

    assert!(result.pushed, "{result:?}");
    assert!(!fixture.a.join(".refine").exists());
    assert!(!git_stdout(&fixture.a, &["ls-remote", "upstream", REFINE_STATE_REF]).is_empty());
}
