use super::*;

fn missing_baseline_fixture(name: &str) -> SyncFixture {
    let fixture = SyncFixture::new(name);
    write_goal(&fixture.a, "REMOTE");
    fixture.service(&fixture.a).sync().unwrap();
    write_goal(&fixture.b, "LIVE");
    fixture
}

fn write_schedulable_goal_in_refine(refine: &Path, id: &str) {
    let goal = refine.join("goals").join(&id[..2]).join(&id[2..]);
    fs::create_dir_all(&goal).unwrap();
    fs::write(
        goal.join("goal.json"),
        format!(r#"{{"id":"{id}","name":"{id}","status":"todo","rounds":[]}}"#),
    )
    .unwrap();
}

fn write_schedulable_goal(root: &Path, id: &str) {
    write_schedulable_goal_in_refine(&refine_dir_for_target_root(root).unwrap(), id);
}

fn write_shared_state(root: &Path, path: &str, value: &str) {
    let destination = refine_dir_for_target_root(root).unwrap().join(path);
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(destination, value).unwrap();
}

fn valid_baseline_conflict_fixture(name: &str) -> SyncFixture {
    let fixture = SyncFixture::new(name);
    write_shared_state(&fixture.a, "shared/state.json", "base\n");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    write_shared_state(&fixture.a, "shared/state.json", "remote\n");
    write_shared_state(&fixture.a, "shared/remote-only.json", "remote-only\n");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared_state(&fixture.b, "shared/state.json", "live\n");
    write_shared_state(&fixture.b, "shared/live-only.json", "live-only\n");
    fixture.service(&fixture.b).sync().unwrap_err();
    fixture
}

#[test]
fn valid_baseline_conflict_report_is_complete_and_preview_is_exact() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-preview");
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(report.phase, StateSyncConflictPhase::FirstPass);
    assert_eq!(report.unresolved_paths, vec!["shared/state.json"]);
    assert!(std::path::Path::new(&report.report_location).is_file());
    assert!(
        !std::path::Path::new(&report.report_location)
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .and_then(|value| value.to_str())
                == Some("tmp"))
    );

    let preview = fixture
        .service(&fixture.b)
        .preview_state_recovery()
        .unwrap();
    assert_eq!(preview.version, 2);
    assert_eq!(preview.baseline_status, "valid_conflict");
    assert_eq!(
        preview.conflict_report_id.as_deref(),
        Some(report.report_id.as_str())
    );
    assert_eq!(preview.conflicting_paths, report.unresolved_paths);
    assert_eq!(preview.conflicting_paths_truncated, 0);
}

#[test]
fn valid_baseline_recovery_reports_typed_git_busy_without_mutation() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-git-busy");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let locked_target = fixture.b.clone();
    let holder = std::thread::spawn(move || {
        with_repository_git_lock(&locked_target, || {
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    held_rx.recv().unwrap();

    let error = service
        .apply_state_recovery_decision(
            StateRecoveryDecision::uniform(StateRecoveryAuthority::Live),
            preview.clone(),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        RefineError::StateRecoveryConflict {
            reason: crate::process::supervisor::errors::StateRecoveryConflictReason::GitBusy,
            ..
        }
    ));
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(service.preview_state_recovery().unwrap(), preview);
}

#[test]
fn valid_baseline_conflict_report_keeps_every_path_while_the_error_stays_bounded() {
    let fixture = SyncFixture::new("reported-recovery-complete-paths");
    for index in 0..128 {
        write_shared_state(
            &fixture.a,
            &format!("shared/conflict-{index:03}.json"),
            "base\n",
        );
    }
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    for index in 0..128 {
        write_shared_state(
            &fixture.a,
            &format!("shared/conflict-{index:03}.json"),
            "remote\n",
        );
    }
    fixture.service(&fixture.a).sync().unwrap();
    for index in 0..128 {
        write_shared_state(
            &fixture.b,
            &format!("shared/conflict-{index:03}.json"),
            "live\n",
        );
    }

    let error = fixture.service(&fixture.b).sync().unwrap_err().to_string();
    assert!(error.contains("128 unresolved path(s)"), "{error}");
    assert!(
        error.len() < 1_000,
        "bounded error was {} bytes",
        error.len()
    );
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(report.unresolved_paths.len(), 128);
    assert_eq!(
        report.unresolved_paths.last().map(String::as_str),
        Some("shared/conflict-127.json")
    );
}

#[test]
fn valid_baseline_recovery_applies_only_reported_overrides_and_preserves_one_sided_changes() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-overrides");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let result = service
        .apply_state_recovery_decision(
            StateRecoveryDecision {
                default_authority: StateRecoveryAuthority::Remote,
                overrides: vec![StateRecoveryOverride {
                    path: "shared/state.json".to_string(),
                    authority: StateRecoveryAuthority::Live,
                }],
            },
            preview,
        )
        .unwrap();

    assert!(result.ok && result.baseline_created, "{result:#?}");
    let live = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(live.join("shared/state.json")).unwrap(),
        "live\n"
    );
    assert!(live.join("shared/live-only.json").exists());
    assert!(live.join("shared/remote-only.json").exists());
    let manifest: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(&result.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.stage, StateRecoveryStage::Completed);
    assert_eq!(manifest.outcome, StateRecoveryOutcome::Succeeded);
    assert!(
        manifest
            .target_location
            .as_deref()
            .is_some_and(|reference| {
                reference.starts_with("refs/refine/state-recovery-target/")
            })
    );
}

#[test]
fn valid_baseline_recovery_rejects_fabricated_decisions_and_preserves_post_apply_writes() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-live-race");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let fabricated = service
        .apply_state_recovery_decision(
            StateRecoveryDecision {
                default_authority: StateRecoveryAuthority::Remote,
                overrides: vec![StateRecoveryOverride {
                    path: "shared/not-reported.json".to_string(),
                    authority: StateRecoveryAuthority::Live,
                }],
            },
            preview.clone(),
        )
        .unwrap_err();
    assert!(
        fabricated
            .to_string()
            .contains("outside the reported conflict set")
    );
    let duplicate = service
        .apply_state_recovery_decision(
            StateRecoveryDecision {
                default_authority: StateRecoveryAuthority::Remote,
                overrides: vec![
                    StateRecoveryOverride {
                        path: "shared/state.json".to_string(),
                        authority: StateRecoveryAuthority::Live,
                    },
                    StateRecoveryOverride {
                        path: "shared/state.json".to_string(),
                        authority: StateRecoveryAuthority::Remote,
                    },
                ],
            },
            preview.clone(),
        )
        .unwrap_err();
    assert!(duplicate.to_string().contains("supplied more than once"));

    let live_path = refine_dir_for_target_root(&fixture.b)
        .unwrap()
        .join("shared/state.json");
    install_after_recovery_authority_hook(&fixture.b, move || {
        fs::write(live_path, "post-apply\n").unwrap();
    });
    let result = service
        .apply_state_recovery_decision(
            StateRecoveryDecision::uniform(StateRecoveryAuthority::Remote),
            preview,
        )
        .unwrap();
    assert!(result.baseline_created);
    let live = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(live.join("shared/state.json")).unwrap(),
        "post-apply\n"
    );
    let published = service.sync().unwrap();
    assert!(published.committed && published.pushed, "{published:#?}");
}

#[test]
fn valid_baseline_recovery_rejects_missing_baseline_bytes_before_owned_side_effects() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-missing-base-bytes");
    let service = fixture.service(&fixture.b);
    rewrite_baseline_fingerprint(&fixture.b, "shared/state.json", u64::MAX);
    service.sync().unwrap_err();
    let preview = service.preview_state_recovery().unwrap();
    let refs_before = git_stdout(&fixture.b, &["show-ref"]);

    let error = service
        .apply_state_recovery_decision(
            StateRecoveryDecision::uniform(StateRecoveryAuthority::Remote),
            preview.clone(),
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("cannot reproduce baseline bytes")
    );
    assert_eq!(git_stdout(&fixture.b, &["show-ref"]), refs_before);
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join("refine-state-recoveries")
            .join(format!("{}-reported.json", preview.evidence_id))
            .exists()
    );
}

#[test]
fn valid_baseline_recovery_rejects_stale_preview_before_side_effects() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-stale");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    write_shared_state(&fixture.b, "shared/later.json", "later\n");
    let error = service
        .apply_state_recovery_decision(
            StateRecoveryDecision::uniform(StateRecoveryAuthority::Remote),
            preview,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("live state snapshot changed at shared/later.json")
    );
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join("refine-state-recoveries")
            .exists()
    );
}

#[test]
fn valid_baseline_preview_reports_the_path_changed_since_the_conflict() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-stale-before-preview");
    let service = fixture.service(&fixture.b);
    write_shared_state(&fixture.b, "shared/later.json", "later\n");

    let error = service.preview_state_recovery().unwrap_err();

    assert!(
        error
            .to_string()
            .contains("live state snapshot changed at shared/later.json"),
        "{error}"
    );
}

#[test]
fn valid_baseline_recovery_resumes_only_its_exact_manifest_and_owned_target() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-resume");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let baseline_path = git_common_dir(&fixture.b)
        .unwrap()
        .join(STATE_BASELINE_FILE);
    let original_baseline = fs::read(&baseline_path).unwrap();
    let remote = fixture.root.join("remote.git");
    let hook_remote = remote.clone();
    install_after_recovery_authority_hook(&fixture.b, move || {
        git(&hook_remote, &["update-ref", "-d", REFINE_STATE_REF]);
    });
    let decision = StateRecoveryDecision::uniform(StateRecoveryAuthority::Remote);
    let interrupted = service
        .apply_state_recovery_decision(decision.clone(), preview.clone())
        .unwrap_err();
    assert!(
        interrupted
            .to_string()
            .contains("moved from the owned target")
    );
    let manifest_path = git_common_dir(&fixture.b)
        .unwrap()
        .join("refine-state-recoveries")
        .join(format!("{}-reported.json", preview.evidence_id));
    let manifest: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.stage, StateRecoveryStage::Hydrated);
    assert_eq!(fs::read(&baseline_path).unwrap(), original_baseline);
    fs::remove_file(preview.conflict_report_location.as_ref().unwrap()).unwrap();

    let different = service
        .apply_state_recovery_decision(
            StateRecoveryDecision::uniform(StateRecoveryAuthority::Live),
            preview.clone(),
        )
        .unwrap_err();
    assert!(
        different
            .to_string()
            .contains("exact owned preview and decision set")
    );

    let target_head = manifest.local_state_head_after.unwrap();
    git(&remote, &["update-ref", REFINE_STATE_REF, &target_head]);
    let resumed = service
        .apply_state_recovery_decision(decision, preview)
        .unwrap();
    assert!(resumed.ok && resumed.baseline_created, "{resumed:#?}");
    let completed: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(completed.stage, StateRecoveryStage::Completed);
}

#[test]
fn valid_baseline_recovery_resumes_the_exact_manifest_after_baseline_interruption() {
    let fixture = valid_baseline_conflict_fixture("reported-recovery-baseline-interruption");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let decision = StateRecoveryDecision::uniform(StateRecoveryAuthority::Remote);
    install_after_recovery_baseline_hook(&fixture.b);

    let interrupted = service
        .apply_state_recovery_decision(decision.clone(), preview.clone())
        .unwrap_err();
    assert!(
        interrupted.to_string().contains("simulated interruption"),
        "{interrupted}"
    );
    let manifest_path = git_common_dir(&fixture.b)
        .unwrap()
        .join("refine-state-recoveries")
        .join(format!("{}-reported.json", preview.evidence_id));
    let interrupted_manifest: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(interrupted_manifest.stage, StateRecoveryStage::Hydrated);
    assert!(
        git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );

    let mut premature = interrupted_manifest.clone();
    premature.stage = StateRecoveryStage::Published;
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&premature).unwrap(),
    )
    .unwrap();
    let premature_error = service
        .apply_state_recovery_decision(decision.clone(), preview.clone())
        .unwrap_err();
    assert!(
        premature_error
            .to_string()
            .contains("before the manifest recorded its hydration boundary"),
        "{premature_error}"
    );
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&interrupted_manifest).unwrap(),
    )
    .unwrap();

    let different = service
        .apply_state_recovery_decision(
            StateRecoveryDecision::uniform(StateRecoveryAuthority::Live),
            preview.clone(),
        )
        .unwrap_err();
    assert!(
        different
            .to_string()
            .contains("exact owned preview and decision set")
    );

    let resumed = service
        .apply_state_recovery_decision(decision, preview)
        .unwrap();
    assert!(resumed.ok && resumed.baseline_created, "{resumed:#?}");
    let completed: StateRecoveryManifest =
        serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(completed.stage, StateRecoveryStage::Completed);
    assert_eq!(completed.outcome, StateRecoveryOutcome::Succeeded);
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
fn concurrent_sync_owned_live_write_waits_for_preview_and_reports_the_changed_path() {
    let fixture = missing_baseline_fixture("recovery-concurrent-sync-writer");
    let service = fixture.service(&fixture.b);
    let target = fixture.b.clone();
    let (preview_locked_tx, preview_locked_rx) = std::sync::mpsc::channel();
    let (release_preview_tx, release_preview_rx) = std::sync::mpsc::channel();
    install_during_recovery_preview_hook(&target, move || {
        preview_locked_tx.send(()).unwrap();
        release_preview_rx.recv().unwrap();
    });

    let (writer_started_tx, writer_started_rx) = std::sync::mpsc::channel();
    let (writer_done_tx, writer_done_rx) = std::sync::mpsc::channel();
    let preview = thread::scope(|scope| {
        let preview_handle = scope.spawn(|| service.preview_state_recovery().unwrap());
        preview_locked_rx.recv().unwrap();
        let writer_target = target.clone();
        let writer = scope.spawn(move || {
            writer_started_tx.send(()).unwrap();
            with_repository_git_lock(&writer_target, || {
                write_goal(&writer_target, "CONCURRENT");
                Ok(())
            })
            .unwrap();
            writer_done_tx.send(()).unwrap();
        });
        writer_started_rx.recv().unwrap();
        assert!(
            writer_done_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "the verified writer must wait while preview owns snapshot capture"
        );
        release_preview_tx.send(()).unwrap();
        let preview = preview_handle.join().unwrap();
        writer.join().unwrap();
        writer_done_rx.recv().unwrap();
        preview
    });

    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("goals/CONCURRENT/goal.json"), "{message}");
}

#[test]
fn node_local_state_sync_activity_is_outside_the_recovery_snapshot_fence() {
    use crate::tools::observability::activity::{ActivityService, FileActivityService};

    let fixture = missing_baseline_fixture("recovery-state-sync-activity-boundary");
    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    let activity = FileActivityService::new(refine_dir_for_target_root(&fixture.b).unwrap());
    activity
        .append(activity.new_entry(
            "State sync remains unavailable",
            "warn",
            "state_sync",
            None,
            Some("refine".to_string()),
        ))
        .unwrap();

    let result = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap();

    assert!(result.ok && result.baseline_created, "{result:#?}");
}

#[test]
fn ordinary_sync_fails_closed_with_actionable_recovery_and_a_clean_state_worktree() {
    let fixture = missing_baseline_fixture("recovery-fail-closed");
    let error = fixture.service(&fixture.b).sync().unwrap_err();
    assert!(matches!(&error, RefineError::StateSyncMissingBaseline(_)));
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
fn remote_authority_updates_the_scheduler_index_with_hydrated_goals() {
    let fixture = SyncFixture::new("recovery-remote-active-index");
    write_schedulable_goal(&fixture.a, "REMOTEGOAL");
    fixture.service(&fixture.a).sync().unwrap();
    write_schedulable_goal(&fixture.b, "LIVEGOAL00");
    let live = refine_dir_for_target_root(&fixture.b).unwrap();
    let before = ActiveGoalIndex::load_or_rebuild(&live).unwrap();
    assert_eq!(
        before
            .goals()
            .map(|goal| goal.id.as_str())
            .collect::<Vec<_>>(),
        vec!["LIVEGOAL00"]
    );

    let service = fixture.service(&fixture.b);
    let preview = service.preview_state_recovery().unwrap();
    service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap();

    let after = ActiveGoalIndex::load_or_rebuild(&live).unwrap();
    assert_eq!(
        after
            .goals()
            .map(|goal| goal.id.as_str())
            .collect::<Vec<_>>(),
        vec!["REMOTEGOAL"]
    );
}

#[test]
fn recovery_rejects_a_rehashed_preview_with_fabricated_comparison_evidence() {
    let fixture = missing_baseline_fixture("recovery-fabricated-comparison");
    let service = fixture.service(&fixture.b);
    let mut preview = service.preview_state_recovery().unwrap();
    preview.path_counts.live_only += 1;
    preview.evidence_id =
        crate::tools::host::git_sync::recovery::preview_evidence_id_for_test(&preview).unwrap();

    let error = service
        .apply_state_recovery(StateRecoveryAuthority::Remote, preview)
        .unwrap_err();

    assert!(
        error.to_string().contains("bounded path comparison"),
        "{error}"
    );
    assert!(
        !git_common_dir(&fixture.b)
            .unwrap()
            .join(STATE_BASELINE_FILE)
            .exists()
    );
    assert_eq!(
        git_stdout(
            &fixture.b,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/refine/state-recovery",
            ]
        ),
        "",
        "invalid comparison evidence must be rejected before a recovery ref is created"
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
    assert!(matches!(
        live_error,
        RefineError::StateRecoveryConflict {
            reason: crate::process::supervisor::errors::StateRecoveryConflictReason::StalePreview,
            ..
        }
    ));
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
    assert!(matches!(
        error,
        RefineError::StateRecoveryConflict {
            reason: crate::process::supervisor::errors::StateRecoveryConflictReason::GitBusy,
            ..
        }
    ));
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

#[test]
fn remote_hydration_retry_repairs_the_scheduler_index_after_files_settle() {
    let root = unique_temp_dir("recovery-hydration-index-retry");
    let original = root.join("original");
    let remote = root.join("remote");
    let live = root.join("live");
    for path in [&original, &remote, &live] {
        fs::create_dir_all(path).unwrap();
    }
    write_schedulable_goal_in_refine(&original, "LIVEGOAL00");
    write_schedulable_goal_in_refine(&live, "LIVEGOAL00");
    write_schedulable_goal_in_refine(&remote, "REMOTEGOAL");
    let before = ActiveGoalIndex::load_or_rebuild(&live).unwrap();
    assert_eq!(before.goals().next().unwrap().id, "LIVEGOAL00");

    // Stand in for interruption after file replacement but before the
    // scheduler-index append/forget operations.
    fs::remove_dir_all(live.join("goals")).unwrap();
    write_schedulable_goal_in_refine(&live, "REMOTEGOAL");
    hydrate_remote_with_recovery_cas(&original, &remote, &live).unwrap();

    let after = ActiveGoalIndex::load_or_rebuild(&live).unwrap();
    assert_eq!(
        after
            .goals()
            .map(|goal| goal.id.as_str())
            .collect::<Vec<_>>(),
        vec!["REMOTEGOAL"]
    );
    fs::remove_dir_all(root).unwrap();
}
