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
fn push_retry_rechecks_original_base_against_fresh_local_and_remote_state() {
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
        errors[0]
            .to_string()
            .contains("during push_retry: 1 unresolved path"),
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
fn missing_first_reconciliation_requires_explicit_authority_without_deleting_remote_records() {
    let fixture = SyncFixture::new("failed-first-reconciliation");
    write_goal(&fixture.a, "GOALA");
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    fs::create_dir_all(&refine_b).unwrap();
    fs::write(refine_a.join("shared.json"), "{\"node\":\"a\"}\n").unwrap();
    fs::write(refine_b.join("shared.json"), "{\"node\":\"b\"}\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    let error = fixture.service(&fixture.b).sync().unwrap_err();
    assert!(error.to_string().contains("baseline is missing"), "{error}");
    assert!(refine_b.join("shared.json").exists());
    assert!(
        !git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/goals/GOALA/goal.json"],
        )
        .is_empty()
    );

    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let recovered = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap();

    assert!(recovered.baseline_created, "{recovered:?}");
    assert!(refine_b.join("goals/GOALA/goal.json").exists());
    assert!(
        !git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/goals/GOALA/goal.json"],
        )
        .is_empty()
    );
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
