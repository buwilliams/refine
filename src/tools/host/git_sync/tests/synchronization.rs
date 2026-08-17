use super::*;

fn valid_goal_record(id: &str, node_id: &str, status: &str, title: &str, updated: &str) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "id": id,
        "name": id,
        "title": title,
        "status": status,
        "priority": "low",
        "reporter": null,
        "branch_name": null,
        "feature_id": null,
        "feature_order": null,
        "node_id": node_id,
        "created": "2026-08-03T18:00:00Z",
        "updated": updated,
        "notes": [],
        "rounds": []
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn sync_merges_disjoint_state_when_nodes_race() {
    let fixture = SyncFixture::new("race");
    write_goal(&fixture.a, "SEED");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
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
fn sync_converges_disjoint_node_heartbeats_without_a_conflict_report() {
    let fixture = SyncFixture::new("node-heartbeat-merge");
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
    fixture.service(&fixture.a).sync().unwrap();
    write_nodes(
        &fixture.b,
        &[
            ("node-a", "2026-08-17T08:00:00Z", "unknown"),
            ("node-b", "2026-08-17T08:02:00Z", "healthy"),
        ],
    );

    let result = fixture.service(&fixture.b).sync().unwrap();

    assert!(
        result.committed && result.pulled && result.pushed,
        "{result:?}"
    );
    assert!(
        result.detail.as_deref().is_some_and(
            |detail| detail.contains("Merged divergent state records structurally: nodes.json")
        ),
        "{result:?}"
    );
    let nodes = read_nodes(&fixture.b);
    assert_eq!(nodes.nodes[0].updated_at, "2026-08-17T08:01:00Z");
    assert_eq!(nodes.nodes[1].updated_at, "2026-08-17T08:02:00Z");
    assert!(
        latest_state_sync_conflict_report(&fixture.b.join("run"))
            .unwrap()
            .is_none()
    );
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
    assert!(error.to_string().contains("1 unresolved path"), "{error}");
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(report.unresolved_paths, vec!["goals/GOALA/goal.json"]);
}

#[test]
fn contested_member_conflict_keeps_a_stable_report_identity_across_attempts() {
    let fixture = SyncFixture::new("stable-report-id");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    let goal_a = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GOALA/goal.json");
    let goal_b = refine_dir_for_target_root(&fixture.b)
        .unwrap()
        .join("goals/GOALA/goal.json");
    fs::write(
        &goal_a,
        valid_goal_record("GOALA", "node-a", "review", "Base", "2026-08-03T18:21:00Z"),
    )
    .unwrap();
    fs::write(
        &goal_b,
        valid_goal_record("GOALA", "node-a", "qa", "Base", "2026-08-03T18:22:00Z"),
    )
    .unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    fixture.service(&fixture.b).sync().unwrap_err();
    let first = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    // Nothing changed between attempts: the same divergence keeps the same
    // report identity, and the summary names the contested member.
    fixture.service(&fixture.b).sync().unwrap_err();
    let second = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(first.report_id, second.report_id);
    assert!(
        first
            .conflicts
            .iter()
            .any(|conflict| conflict.summary.contains("goal GOALA")
                && conflict.summary.contains("status")),
        "{:?}",
        first.conflicts
    );
}

#[test]
fn sync_merges_disjoint_goal_changes_without_blocking_remote_records() {
    let fixture = SyncFixture::new("disjoint-goal-fields");
    write_goal(&fixture.a, "GOALA");
    let goal_a = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GOALA/goal.json");
    fs::write(
        &goal_a,
        valid_goal_record("GOALA", "node-a", "review", "Base", "2026-08-03T18:20:00Z"),
    )
    .unwrap();
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    fs::write(
        &goal_a,
        valid_goal_record("GOALA", "node-b", "review", "Base", "2026-08-03T18:21:00Z"),
    )
    .unwrap();
    write_goal(&fixture.a, "REMOTE_ONLY");
    fixture.service(&fixture.a).sync().unwrap();

    let goal_b = refine_dir_for_target_root(&fixture.b)
        .unwrap()
        .join("goals/GOALA/goal.json");
    fs::write(
        &goal_b,
        valid_goal_record("GOALA", "node-b", "done", "Base", "2026-08-03T18:22:00Z"),
    )
    .unwrap();

    let result = fixture.service(&fixture.b).sync().unwrap();

    assert!(
        result.committed && result.pulled && result.pushed,
        "{result:?}"
    );
    assert!(
        result
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("Merged divergent state records structurally")),
        "{result:?}"
    );
    let merged: serde_json::Value = serde_json::from_slice(&fs::read(&goal_b).unwrap()).unwrap();
    assert_eq!(merged["status"], "done");
    assert_eq!(merged["node_id"], "node-b");
    assert_eq!(merged["updated"], "2026-08-03T18:22:00Z");
    assert!(
        refine_dir_for_target_root(&fixture.b)
            .unwrap()
            .join("goals/REMOTE_ONLY/goal.json")
            .exists()
    );
}

#[test]
fn unresolved_conflict_withholds_driver_resolved_merges() {
    let fixture = SyncFixture::new("mixed-goal-conflicts");
    write_goal(&fixture.a, "GOALA");
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    fs::write(
        refine_a.join("goals/GOALA/goal.json"),
        valid_goal_record("GOALA", "node-a", "backlog", "Base", "2026-08-03T18:20:00Z"),
    )
    .unwrap();
    fs::create_dir_all(refine_a.join("shared")).unwrap();
    fs::write(refine_a.join("shared/state.json"), "base\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    fs::write(
        refine_a.join("goals/GOALA/goal.json"),
        valid_goal_record("GOALA", "node-a", "todo", "Base", "2026-08-03T18:21:00Z"),
    )
    .unwrap();
    fs::write(refine_a.join("shared/state.json"), "remote\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();
    let remote_head_before = git_stdout(&fixture.a, &["rev-parse", "origin/refine/state"]);

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    fs::write(
        refine_b.join("goals/GOALA/goal.json"),
        valid_goal_record(
            "GOALA",
            "node-b",
            "backlog",
            "Retitled",
            "2026-08-03T18:22:00Z",
        ),
    )
    .unwrap();
    fs::write(refine_b.join("shared/state.json"), "live\n").unwrap();

    // The Goal contention is driver-resolvable (disjoint members), but the
    // shared record is not; one unresolved path withholds the entire merge.
    let _error = fixture.service(&fixture.b).sync().unwrap_err();

    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(
        report.unresolved_paths,
        vec!["shared/state.json".to_string()]
    );
    git(&fixture.b, &["fetch", "-q", "origin", REFINE_STATE_BRANCH]);
    assert_eq!(
        git_stdout(&fixture.b, &["rev-parse", "origin/refine/state"]),
        remote_head_before,
        "a conflicted pass must publish nothing"
    );
    assert_eq!(
        fs::read_to_string(refine_b.join("shared/state.json")).unwrap(),
        "live\n"
    );
    assert_eq!(
        git_stdout(
            &state_worktree_for_target_root(&fixture.b).unwrap(),
            &["status", "--short"]
        ),
        ""
    );
}

#[test]
fn rejected_publish_then_fast_forward_push_without_a_conflict_report() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = SyncFixture::new("ancestor-fast-forward");
    write_goal(&fixture.a, "GOALA");
    fixture.service(&fixture.a).sync().unwrap();

    // The origin rejects state pushes; the advance still commits locally.
    let remote = fixture.root.join("remote.git");
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state; then exit 1; fi\ndone\nexit 0\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).unwrap();

    let live_goal = refine_dir_for_target_root(&fixture.a)
        .unwrap()
        .join("goals/GOALA/goal.json");
    fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"v2\"}\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap_err();
    assert!(
        latest_state_sync_conflict_report(&fixture.a.join("run"))
            .unwrap()
            .is_none(),
        "a one-sided local advance must not produce a conflict report"
    );
    let origin_head = git_stdout(&fixture.a, &["rev-parse", "origin/refine/state"]);
    let local_head = git_stdout(&fixture.a, &["rev-parse", "refine/state"]);
    assert_ne!(origin_head, local_head);
    git(
        &fixture.a,
        &["merge-base", "--is-ancestor", &origin_head, &local_head],
    );

    // Remote is a strict ancestor of local: the next pass fast-forwards and
    // publishes without merging.
    fs::remove_file(&hook).unwrap();
    fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"v3\"}\n").unwrap();
    let result = fixture.service(&fixture.a).sync().unwrap();
    assert!(result.pushed, "{result:?}");
    assert!(
        latest_state_sync_conflict_report(&fixture.a.join("run"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        git_stdout(&fixture.a, &["rev-parse", "origin/refine/state"]),
        git_stdout(&fixture.a, &["rev-parse", "refine/state"])
    );
}

#[test]
fn hydration_preimage_cas_skips_records_the_daemon_advanced_mid_pass() {
    let fixture = SyncFixture::new("hydration-cas");
    write_goal(&fixture.a, "GOALA");
    let service = fixture.service(&fixture.a);
    service.sync().unwrap();
    let live_refine = refine_dir_for_target_root(&fixture.a).unwrap();
    let original = durable_state_map(&live_refine).unwrap();
    let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
    let base_commit = git_stdout(&state_worktree, &["rev-parse", "HEAD"]);

    // Build the adopted commit: GOALA advanced remotely, GOALB added.
    fs::write(
        state_worktree.join(".refine/goals/GOALA/goal.json"),
        "{\"id\":\"GOALA\",\"status\":\"remote\"}\n",
    )
    .unwrap();
    fs::create_dir_all(state_worktree.join(".refine/goals/GOALB")).unwrap();
    fs::write(
        state_worktree.join(".refine/goals/GOALB/goal.json"),
        "{\"id\":\"GOALB\"}\n",
    )
    .unwrap();
    git(&state_worktree, &["add", "-f", "-A", "--", ".refine"]);
    git(&state_worktree, &["commit", "-q", "-m", "advance"]);
    let target_commit = git_stdout(&state_worktree, &["rev-parse", "HEAD"]);

    // A daemon write lands after the pass snapshotted live.
    let live_goal = live_refine.join("goals/GOALA/goal.json");
    fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"concurrent\"}\n").unwrap();

    let concurrent = service
        .hydrate_live_from_commit(
            &state_worktree,
            crate::tools::host::git_sync::service::HydrationScope::ChangedSince(base_commit),
            &target_commit,
            &original,
            &live_refine,
        )
        .unwrap();

    assert!(concurrent, "the skipped record must be reported");
    assert_eq!(
        fs::read_to_string(&live_goal).unwrap(),
        "{\"id\":\"GOALA\",\"status\":\"concurrent\"}\n",
        "a record the daemon advanced mid-pass is left alone"
    );
    assert_eq!(
        fs::read_to_string(live_refine.join("goals/GOALB/goal.json")).unwrap(),
        "{\"id\":\"GOALB\"}\n",
        "unchanged-preimage records hydrate normally"
    );
}

#[test]
fn first_sync_retires_the_legacy_baseline_file_and_anchor_refs() {
    let fixture = SyncFixture::new("legacy-baseline-retire");
    write_goal(&fixture.a, "GOALA");
    let baseline = git_common_dir(&fixture.a)
        .unwrap()
        .join("refine-state-baseline.json");
    fs::write(&baseline, "{\"legacy\":true}\n").unwrap();
    let head = git_stdout(&fixture.a, &["rev-parse", "HEAD"]);
    git(
        &fixture.a,
        &["update-ref", "refs/refine/state-baseline/legacy", &head],
    );

    fixture.service(&fixture.a).sync().unwrap();

    assert!(!baseline.exists());
    assert_eq!(
        git_stdout(
            &fixture.a,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/refine/state-baseline",
            ],
        ),
        ""
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
