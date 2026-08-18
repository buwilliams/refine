use super::*;

fn write_shared_state(root: &Path, path: &str, value: &str) {
    let refine = refine_dir_for_target_root(root).unwrap();
    let destination = refine.join(path);
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(destination, value).unwrap();
}

/// Two nodes that share history and then contest one record.
fn diverged_fixture(name: &str) -> SyncFixture {
    let fixture = SyncFixture::new(name);
    write_shared_state(&fixture.a, "shared/state.json", "{\"value\":\"base\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    write_shared_state(&fixture.a, "shared/state.json", "{\"value\":\"remote\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared_state(&fixture.b, "shared/state.json", "{\"value\":\"live\"}\n");
    fixture
}

#[test]
fn run_synchronizes_without_recovery_when_nothing_conflicts() {
    let fixture = SyncFixture::new("run-clean");
    write_goal(&fixture.a, "GOALA");
    let run = fixture
        .service(&fixture.a)
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(run.ok && !run.recovered, "{run:?}");
    assert_eq!(run.attempts, 1);
    assert!(run.sync.pushed, "{run:?}");
    assert!(run.recovery.is_none());
}

#[test]
fn run_without_authority_is_the_ordinary_pipeline_and_fails_closed() {
    let fixture = diverged_fixture("run-sync-only");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .expect("the contested sync records a conflict report");

    let error = service
        .run_state_recovery_with_policy(StateRecoveryRunPolicy::SyncOnly)
        .unwrap_err();
    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    let rerun_report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(
        report.report_id, rerun_report.report_id,
        "the same divergence keeps its stable report id"
    );
}

#[test]
fn run_recovers_a_two_sided_conflict_with_remote_authority_in_one_call() {
    let fixture = diverged_fixture("run-remote-authority");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();
    let local_head_before = git_stdout(&fixture.b, &["rev-parse", "refine/state"]);

    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");
    let recovery = run.recovery.as_ref().unwrap();
    assert_eq!(
        recovery.settled_paths,
        vec!["shared/state.json".to_string()]
    );
    assert!(recovery.retained_refs.is_empty(), "{recovery:?}");
    assert_eq!(recovery.local_state_head, recovery.remote_state_head);

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(refine_b.join("shared/state.json")).unwrap(),
        "{\"value\":\"remote\"}\n"
    );
    // Both pre-merge heads are parents of the recovery merge commit, so the
    // displaced live version stays reachable without any retained ref.
    let head = git_stdout(&fixture.b, &["rev-parse", "refine/state"]);
    let parents = git_stdout(&fixture.b, &["rev-parse", &format!("{head}^@")]);
    assert!(parents.lines().any(|parent| parent == local_head_before));
    assert_eq!(
        git_stdout(
            &fixture.b,
            &[
                "show",
                &format!("{local_head_before}:.refine/shared/state.json")
            ],
        ),
        "{\"value\":\"live\"}"
    );
    // The fleet converges after the other node syncs.
    let synced = fixture.service(&fixture.a).sync().unwrap();
    assert!(synced.ok, "{synced:?}");
    assert_eq!(
        git_stdout(&fixture.a, &["rev-parse", "refine/state"]),
        git_stdout(&fixture.b, &["rev-parse", "refine/state"])
    );

    // Rerunning the command it exists to clear is a no-op with no new report.
    let report_before = fs::read(fixture.b.join("run/state-sync-conflicts/latest.json")).ok();
    let rerun = service
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(rerun.ok && !rerun.recovered, "{rerun:?}");
    assert_eq!(rerun.attempts, 1);
    assert_eq!(
        fs::read(fixture.b.join("run/state-sync-conflicts/latest.json")).ok(),
        report_before
    );
}

#[test]
fn run_keeps_one_sided_changes_while_authority_settles_contested_paths() {
    let fixture = diverged_fixture("run-keeps-one-sided");
    // A one-sided addition on the losing node must survive remote authority.
    write_goal(&fixture.b, "GOALB");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();

    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");
    assert_eq!(
        run.recovery.as_ref().unwrap().settled_paths,
        vec!["shared/state.json".to_string()],
        "one-sided work is merged, never settled by authority"
    );

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(refine_b.join("shared/state.json")).unwrap(),
        "{\"value\":\"remote\"}\n"
    );
    assert!(refine_b.join("goals/GOALB/goal.json").exists());
    assert!(
        !git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/goals/GOALB/goal.json"],
        )
        .is_empty(),
        "the one-sided record must be published, not discarded by authority"
    );
}

#[test]
fn run_live_authority_keeps_the_local_side_and_converges_the_fleet() {
    let fixture = diverged_fixture("run-live-authority");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();

    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(StateRecoveryAuthority::Live))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(refine_b.join("shared/state.json")).unwrap(),
        "{\"value\":\"live\"}\n"
    );
    fixture.service(&fixture.a).sync().unwrap();
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    assert_eq!(
        fs::read_to_string(refine_a.join("shared/state.json")).unwrap(),
        "{\"value\":\"live\"}\n"
    );
}

#[test]
fn run_honors_per_path_overrides_against_the_default_authority() {
    let fixture = SyncFixture::new("run-overrides");
    write_shared_state(&fixture.a, "first.json", "{\"value\":\"base\"}\n");
    write_shared_state(&fixture.a, "second.json", "{\"value\":\"base\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    write_shared_state(&fixture.a, "first.json", "{\"value\":\"remote\"}\n");
    write_shared_state(&fixture.a, "second.json", "{\"value\":\"remote\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared_state(&fixture.b, "first.json", "{\"value\":\"live\"}\n");
    write_shared_state(&fixture.b, "second.json", "{\"value\":\"live\"}\n");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();

    let run = service
        .run_state_recovery(StateRecoveryDecision {
            default_authority: StateRecoveryAuthority::Remote,
            overrides: vec![StateRecoveryOverride {
                path: "second.json".to_string(),
                authority: StateRecoveryAuthority::Live,
            }],
        })
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(refine_b.join("first.json")).unwrap(),
        "{\"value\":\"remote\"}\n"
    );
    assert_eq!(
        fs::read_to_string(refine_b.join("second.json")).unwrap(),
        "{\"value\":\"live\"}\n"
    );
    assert_eq!(
        git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/second.json"]
        ),
        "{\"value\":\"live\"}"
    );
}

#[test]
fn join_recovery_with_remote_authority_retains_displaced_live_state() {
    let fixture = SyncFixture::new("join-recovery-remote");
    write_shared_state(&fixture.a, "shared.json", "{\"node\":\"a\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared_state(&fixture.b, "shared.json", "{\"node\":\"b\"}\n");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();

    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");
    let recovery = run.recovery.as_ref().unwrap();
    assert_eq!(recovery.settled_paths, vec!["shared.json".to_string()]);
    let retained = recovery
        .retained_refs
        .first()
        .expect("a joining node's displaced live store is retained by ref");
    assert!(
        retained.starts_with("refs/refine/retained/live-"),
        "{recovery:?}"
    );
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(refine_b.join("shared.json")).unwrap(),
        "{\"node\":\"a\"}\n"
    );
    assert_eq!(
        git_stdout(
            &fixture.b,
            &["show", &format!("{retained}:.refine/shared.json")],
        ),
        "{\"node\":\"b\"}"
    );
    // Subsequent syncs are ordinary.
    let synced = service.sync().unwrap();
    assert!(synced.ok && !synced.deferred, "{synced:?}");
}

#[test]
fn join_recovery_with_live_authority_publishes_live_and_keeps_remote_records() {
    let fixture = SyncFixture::new("join-recovery-live");
    write_goal(&fixture.a, "GOALA");
    write_shared_state(&fixture.a, "shared.json", "{\"node\":\"a\"}\n");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared_state(&fixture.b, "shared.json", "{\"node\":\"b\"}\n");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();

    let run = service
        .run_state_recovery(StateRecoveryDecision::uniform(StateRecoveryAuthority::Live))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");
    assert!(
        run.recovery.as_ref().unwrap().retained_refs.is_empty(),
        "live authority displaces nothing that is not already a commit ancestor"
    );

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    assert_eq!(
        fs::read_to_string(refine_b.join("shared.json")).unwrap(),
        "{\"node\":\"b\"}\n"
    );
    assert!(
        refine_b.join("goals/GOALA/goal.json").exists(),
        "remote-only records must be preserved and hydrated"
    );
    assert_eq!(
        git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/shared.json"],
        ),
        "{\"node\":\"b\"}"
    );
    assert!(
        !git_stdout(
            &fixture.b,
            &["show", "origin/refine/state:.refine/goals/GOALA/goal.json"],
        )
        .is_empty()
    );
}

#[test]
fn recovery_rejects_foreign_git_operation_markers() {
    let fixture = diverged_fixture("foreign-markers");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();

    let marker = git_common_dir(&fixture.b).unwrap().join("MERGE_HEAD");
    fs::write(&marker, "0000000000000000000000000000000000000000\n").unwrap();
    let error = service
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap_err();
    assert!(
        error.to_string().contains("Git operation marker"),
        "{error}"
    );
    fs::remove_file(&marker).unwrap();

    let recovered = service
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(recovered.ok && recovered.recovered, "{recovered:?}");
}

#[test]
fn preview_reports_a_diverged_head_pair_with_domain_summaries() {
    let fixture = diverged_fixture("preview-diverged");
    let service = fixture.service(&fixture.b);
    service.sync().unwrap_err();
    // A one-sided live change that has not reached the branch yet.
    write_shared_state(&fixture.b, "pending.json", "{\"value\":\"pending\"}\n");

    let preview = service.preview_state_recovery().unwrap();
    assert_eq!(preview.ancestry, "diverged", "{preview:?}");
    assert!(preview.merge_base.is_some());
    assert_eq!(
        preview
            .conflicts
            .iter()
            .map(|conflict| conflict.path.as_str())
            .collect::<Vec<_>>(),
        vec!["shared/state.json"]
    );
    assert!(
        preview.conflicts[0].summary.contains("value"),
        "the summary names the contested member: {}",
        preview.conflicts[0].summary
    );
    assert!(
        preview
            .live_pending_paths
            .contains(&"pending.json".to_string()),
        "{preview:?}"
    );
    // Read-only: no artifact files and no state movement.
    assert!(!fixture.b.join("run/refine-state-recoveries").exists());
    assert!(!fixture.b.join(".git/refine-state-recoveries").exists());
}

#[test]
fn preview_reports_a_contested_join_without_a_merge_base() {
    let fixture = SyncFixture::new("preview-join");
    write_shared_state(&fixture.a, "shared.json", "{\"node\":\"a\"}\n");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    write_shared_state(&fixture.b, "shared.json", "{\"node\":\"b\"}\n");
    write_goal(&fixture.b, "GOALB");

    let preview = fixture
        .service(&fixture.b)
        .preview_state_recovery()
        .unwrap();
    assert_eq!(preview.ancestry, "join", "{preview:?}");
    assert!(preview.local_state_head.is_none());
    assert!(preview.merge_base.is_none());
    assert_eq!(
        preview
            .conflicts
            .iter()
            .map(|conflict| conflict.path.as_str())
            .collect::<Vec<_>>(),
        vec!["shared.json"]
    );
    assert!(
        preview
            .local_paths
            .contains(&"goals/GOALB/goal.json".to_string())
    );
    assert!(
        preview
            .remote_paths
            .contains(&"goals/GOALA/goal.json".to_string())
    );
}

#[test]
fn preview_reports_converged_and_ancestor_shapes_without_conflicts() {
    let fixture = SyncFixture::new("preview-shapes");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    let converged = fixture
        .service(&fixture.b)
        .preview_state_recovery()
        .unwrap();
    assert_eq!(converged.ancestry, "converged", "{converged:?}");
    assert!(converged.conflicts.is_empty());

    // The other node publishes; this node is now behind.
    write_goal(&fixture.a, "GOALX");
    fixture.service(&fixture.a).sync().unwrap();
    let behind = fixture
        .service(&fixture.b)
        .preview_state_recovery()
        .unwrap();
    assert_eq!(behind.ancestry, "remote_ahead", "{behind:?}");
    assert!(
        behind
            .remote_paths
            .contains(&"goals/GOALX/goal.json".to_string())
    );
    assert!(behind.conflicts.is_empty());
}
