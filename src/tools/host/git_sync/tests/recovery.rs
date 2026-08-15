use super::*;

fn missing_baseline_fixture(name: &str) -> SyncFixture {
    let fixture = SyncFixture::new(name);
    write_goal(&fixture.a, "REMOTE");
    fixture.service(&fixture.a).sync().unwrap();
    write_goal(&fixture.b, "LIVE");
    fixture
}

#[test]
fn missing_baseline_preview_is_bounded_and_does_not_mutate_target_state() {
    let fixture = missing_baseline_fixture("recovery-preview");
    let service = fixture.service(&fixture.b);
    let refs_before = git_stdout(&fixture.b, &["show-ref"]);
    let app_head_before = git_stdout(&fixture.b, &["rev-parse", "HEAD"]);
    let remote_before = git_stdout(
        &fixture.a,
        &["ls-remote", "origin", "refs/heads/refine/state"],
    );
    let live_before = durable_state_map(&refine_dir_for_target_root(&fixture.b).unwrap()).unwrap();

    let preview = service.preview_state_recovery().unwrap();

    assert_eq!(preview.baseline_status, "missing");
    assert_eq!(preview.configured_remote, "origin");
    assert!(preview.local_state_head.is_none());
    assert!(preview.path_counts.live_only > 0, "{preview:#?}");
    assert!(preview.path_counts.remote_only > 0, "{preview:#?}");
    assert!(preview.conflicting_paths.len() <= 100);
    assert_eq!(git_stdout(&fixture.b, &["show-ref"]), refs_before);
    assert_eq!(
        git_stdout(&fixture.b, &["rev-parse", "HEAD"]),
        app_head_before
    );
    assert_eq!(
        git_stdout(
            &fixture.a,
            &["ls-remote", "origin", "refs/heads/refine/state"]
        ),
        remote_before
    );
    assert_eq!(
        durable_state_map(&refine_dir_for_target_root(&fixture.b).unwrap()).unwrap(),
        live_before
    );
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );
}

#[test]
fn ordinary_sync_fails_closed_with_actionable_recovery_and_a_clean_state_worktree() {
    let fixture = missing_baseline_fixture("recovery-fail-closed");
    let error = fixture.service(&fixture.b).sync().unwrap_err();
    let message = error.to_string();
    assert!(message.contains("baseline is missing"), "{message}");
    assert!(message.contains("state-recovery preview"), "{message}");
    assert!(message.contains("fail-closed"), "{message}");
    let state_root = state_worktree_for_target_root(&fixture.b).unwrap();
    if state_root.exists() {
        assert_eq!(git_stdout(&state_root, &["status", "--porcelain"]), "");
    }
}

#[test]
fn live_authority_retains_remote_history_and_publishes_remote_only_deletions() {
    let fixture = missing_baseline_fixture("recovery-live");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let remote_before = preview.remote_state_head.clone();

    let result = service
        .apply_state_recovery(StateRecoveryAuthority::Live, preview)
        .unwrap();

    assert!(result.ok && result.baseline_created, "{result:#?}");
    let remote_after = git_stdout(&fixture.b, &["rev-parse", "origin/refine/state"]);
    assert_eq!(result.remote_state_head, remote_after);
    assert_eq!(
        git_stdout(&fixture.b, &["merge-base", &remote_before, &remote_after]),
        remote_before
    );
    assert!(
        !git_stdout(&fixture.b, &["ls-tree", "-r", "--name-only", &remote_after])
            .contains("REMOTE/goal.json")
    );
    assert!(
        git_stdout(
            &fixture.b,
            &[
                "show",
                &format!("{remote_after}:.refine/goals/LIVE/goal.json")
            ]
        )
        .contains("LIVE")
    );
    assert!(
        git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );
    let manifest: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.outcome, StateRecoveryOutcome::Succeeded);
    assert_eq!(manifest.recovery_location, result.recovery_location);
}

#[test]
fn remote_authority_preserves_live_snapshot_before_hydrating_remote() {
    let fixture = missing_baseline_fixture("recovery-remote");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();

    let result = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap();

    let live = refine_dir_for_target_root(&fixture.b).unwrap();
    assert!(!live.join("goals/LIVE/goal.json").exists());
    assert!(live.join("goals/REMOTE/goal.json").exists());
    assert!(
        git_stdout(
            &fixture.b,
            &[
                "show",
                &format!("{}:.refine/goals/LIVE/goal.json", result.recovery_location)
            ]
        )
        .contains("LIVE")
    );
    assert_eq!(
        git_stdout(&fixture.b, &["status", "--porcelain"]),
        "",
        "application worktree changed"
    );
}

#[test]
fn recovery_rejects_stale_live_and_remote_previews_without_a_baseline() {
    let fixture = missing_baseline_fixture("recovery-stale");
    let service = fixture.service(&fixture.b);
    let live_preview = service.preview_state_recovery().unwrap();
    write_goal(&fixture.b, "LATER");
    let live_error = service
        .apply_state_recovery(StateRecoveryAuthority::Live, live_preview)
        .unwrap_err();
    assert!(
        live_error
            .to_string()
            .contains("live state snapshot changed")
    );
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );

    fs::remove_dir_all(refine_dir_for_target_root(&fixture.b).unwrap()).unwrap();
    write_goal(&fixture.b, "LIVE");
    let remote_preview = service.preview_state_recovery().unwrap();
    write_goal(&fixture.a, "REMOTE2");
    fixture.service(&fixture.a).sync().unwrap();
    let remote_error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, remote_preview)
        .unwrap_err();
    assert!(
        remote_error
            .to_string()
            .contains("remote refine/state head changed")
    );
}

#[test]
fn recovery_honors_configured_remote_and_rejects_repository_lock_contention() {
    use crate::process::supervisor::config::FileSettingsService;

    let fixture = missing_baseline_fixture("recovery-configured-remote");
    git(&fixture.b, &["remote", "rename", "origin", "shared"]);
    let live = refine_dir_for_target_root(&fixture.b).unwrap();
    FileSettingsService::with_active_root(&live, fixture.b.join("run"))
        .update(&serde_json::json!({"git_remote": "shared"}))
        .unwrap();
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    assert_eq!(preview.configured_remote, "shared");

    let target = fixture.b.clone();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        with_repository_git_lock(&target, || {
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();
    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap_err();
    assert!(error.to_string().contains("Git operations are busy"));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
}

#[test]
fn failed_publish_keeps_owned_evidence_and_is_retryable_without_a_baseline() {
    let fixture = missing_baseline_fixture("recovery-retry");
    let remote = fixture.root.join("remote.git");
    let reject_once = fixture.root.join("reject-recovery-once");
    fs::write(
        remote.join("hooks/pre-receive"),
        format!(
            "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state && test ! -e \"{}\"; then\n    touch \"{}\"\n    exit 1\n  fi\ndone\nexit 0\n",
            reject_once.display(),
            reject_once.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            remote.join("hooks/pre-receive"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let first = service
        .apply_state_recovery(StateRecoveryAuthority::Live, preview.clone())
        .unwrap_err();
    assert!(first.to_string().contains("git push"), "{first}");
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );

    let result = service
        .apply_state_recovery(StateRecoveryAuthority::Live, preview)
        .unwrap();
    assert!(result.baseline_created, "{result:#?}");
    let manifest: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(result.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.outcome, StateRecoveryOutcome::Succeeded);
}

#[test]
fn post_authority_remote_verification_failure_leaves_no_baseline_and_is_retryable() {
    let fixture = missing_baseline_fixture("recovery-post-authority-remote-race");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let observed_remote_head = preview.remote_state_head.clone();
    let remote = fixture.root.join("remote.git");
    let hook_remote = remote.clone();
    install_after_recovery_authority_hook(&fixture.b, move || {
        git(
            &hook_remote,
            &["update-ref", "-d", "refs/heads/refine/state"],
        );
    });

    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview.clone())
        .unwrap_err();

    assert!(
        error.to_string().contains("disappeared after recovery"),
        "{error}"
    );
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists(),
        "a failed final verification must not publish a baseline"
    );

    git(
        &remote,
        &[
            "update-ref",
            "refs/heads/refine/state",
            &observed_remote_head,
        ],
    );
    let retry = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap();
    assert!(retry.ok && retry.baseline_created, "{retry:#?}");
}

#[test]
fn interruption_after_baseline_finalizes_owned_apply_without_republishing_or_foreign_acceptance() {
    let fixture = missing_baseline_fixture("recovery-post-baseline-interruption");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    install_after_recovery_baseline_hook(&fixture.b);

    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Live, preview.clone())
        .unwrap_err();
    assert!(
        error.to_string().contains("simulated interruption"),
        "{error}"
    );

    let common = git_common_dir(&fixture.b).unwrap();
    let baseline_path = common.join(STATE_BASELINE_FILE);
    let owned_baseline = fs::read(&baseline_path).unwrap();
    let manifest_path = common
        .join("refine-state-recoveries")
        .join(format!("{}-live.json", preview.evidence_id));
    let manifest: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.outcome, StateRecoveryOutcome::Started);
    let published_head = git_stdout(&fixture.b, &["ls-remote", "origin", REFINE_STATE_REF]);

    fs::write(&baseline_path, b"{\"foreign.json\":1}\n").unwrap();
    let foreign_error = service
        .apply_state_recovery(StateRecoveryAuthority::Live, preview.clone())
        .unwrap_err();
    assert!(
        foreign_error
            .to_string()
            .contains("does not match an owned interrupted apply"),
        "{foreign_error}"
    );

    fs::write(&baseline_path, owned_baseline).unwrap();
    let retry = service
        .apply_state_recovery(StateRecoveryAuthority::Live, preview)
        .unwrap();
    assert!(retry.ok && retry.baseline_created, "{retry:#?}");
    assert_eq!(
        git_stdout(&fixture.b, &["ls-remote", "origin", REFINE_STATE_REF]),
        published_head,
        "owned finalization must not republish the state branch"
    );
    let completed: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(completed.outcome, StateRecoveryOutcome::Succeeded);
}

#[test]
fn recovery_rejects_foreign_git_operation_markers() {
    let fixture = missing_baseline_fixture("recovery-foreign-operation");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let marker = git_common_dir(&fixture.b).unwrap().join("MERGE_HEAD");
    fs::write(&marker, "foreign\n").unwrap();

    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap_err();

    assert!(error.to_string().contains("MERGE_HEAD"));
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );
    fs::remove_file(marker).unwrap();
}

#[test]
fn recovery_rejects_an_unsafe_managed_state_worktree() {
    let fixture = missing_baseline_fixture("recovery-unsafe-worktree");
    let service = fixture.service(&fixture.b);
    service.fetch_state_branch("origin").unwrap();
    let live = refine_dir_for_target_root(&fixture.b).unwrap();
    let setup = service
        .ensure_state_worktree("origin", true, &live)
        .unwrap();
    let preview = service.preview_state_recovery().unwrap();
    fs::write(
        setup.path.join(".refine/goals/REMOTE/goal.json"),
        "{\"id\":\"REMOTE\",\"dirty\":true}\n",
    )
    .unwrap();

    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap_err();

    assert!(error.to_string().contains("worktree is unsafe"), "{error}");
    assert_eq!(git_stdout(&setup.path, &["status", "--porcelain"]), "");
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );
}

#[test]
fn remote_hydration_resumes_owned_partial_writes_and_rejects_concurrent_content() {
    let root = unique_temp_dir("recovery-hydration-cas");
    let original = root.join("original");
    let remote = root.join("remote");
    let live = root.join("live");
    for path in [&original, &remote, &live] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(original.join("a.json"), "old-a\n").unwrap();
    fs::write(original.join("b.json"), "old-b\n").unwrap();
    fs::write(remote.join("a.json"), "new-a\n").unwrap();
    fs::write(remote.join("b.json"), "new-b\n").unwrap();
    // Stand in for an interruption after the first atomic path replacement.
    fs::write(live.join("a.json"), "new-a\n").unwrap();
    fs::write(live.join("b.json"), "old-b\n").unwrap();

    hydrate_remote_with_recovery_cas(&original, &remote, &live).unwrap();
    assert_eq!(
        durable_state_map(&live).unwrap(),
        durable_state_map(&remote).unwrap()
    );

    fs::write(live.join("b.json"), "concurrent\n").unwrap();
    let error = hydrate_remote_with_recovery_cas(&original, &remote, &live).unwrap_err();
    assert!(error.to_string().contains("b.json"), "{error}");
    assert_eq!(
        fs::read_to_string(live.join("b.json")).unwrap(),
        "concurrent\n"
    );
    fs::remove_dir_all(root).unwrap();
}
