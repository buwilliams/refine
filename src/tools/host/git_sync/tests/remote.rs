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
            .contains("during push retry: goals/GOALA/goal.json"),
        "{}",
        errors[0]
    );
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
fn failed_first_reconciliation_does_not_turn_remote_records_into_local_deletions() {
    let fixture = SyncFixture::new("failed-first-reconciliation");
    write_goal(&fixture.a, "GOALA");
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    fs::create_dir_all(&refine_b).unwrap();
    fs::write(refine_a.join("shared.json"), "{\"node\":\"a\"}\n").unwrap();
    fs::write(refine_b.join("shared.json"), "{\"node\":\"b\"}\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    let error = fixture.service(&fixture.b).sync().unwrap_err();
    assert!(error.to_string().contains("shared.json"), "{error}");
    fs::copy(refine_a.join("shared.json"), refine_b.join("shared.json")).unwrap();

    let recovered = fixture.service(&fixture.b).sync().unwrap();

    assert!(!recovered.committed, "{recovered:?}");
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
