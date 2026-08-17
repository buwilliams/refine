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
fn sync_rebases_disjoint_state_when_nodes_race() {
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
        result
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("Merged per-node registry changes")),
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
fn sync_converges_nodes_with_unavailable_recorded_baseline_bytes_on_first_pass() {
    let fixture = SyncFixture::new("node-baseline-unavailable-first-pass");
    write_nodes(
        &fixture.a,
        &[
            ("node-a", "2026-08-17T08:00:00Z", "unknown"),
            ("node-b", "2026-08-17T08:00:00Z", "unknown"),
        ],
    );
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    rewrite_baseline_fingerprint(&fixture.b, "nodes.json", u64::MAX);

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
        result.detail.as_deref().is_some_and(|detail| detail
            .contains("Merged per-node registry changes with safe baseline-unavailable fallback")),
        "{result:#?}"
    );
    let nodes = read_nodes(&fixture.b);
    assert_eq!(nodes.nodes[0].updated_at, "2026-08-17T08:01:00Z");
    assert_eq!(nodes.nodes[1].updated_at, "2026-08-17T08:02:00Z");
}

#[test]
fn versioned_baseline_anchor_survives_interruption_and_legacy_metadata_still_loads() {
    let fixture = SyncFixture::new("versioned-baseline-anchor-interruption");
    write_goal(&fixture.a, "GOALA");
    let service = fixture.service(&fixture.a);
    service.sync().unwrap();
    let baseline_path = git_common_dir(&fixture.a)
        .unwrap()
        .join(STATE_BASELINE_FILE);
    let original_bytes = fs::read(&baseline_path).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
    assert_eq!(original["version"], 2);
    let original_ref = original["snapshot_ref"].as_str().unwrap().to_string();
    let original_oid = original["snapshot_oid"].as_str().unwrap().to_string();
    assert_eq!(
        git_stdout(&fixture.a, &["rev-parse", &original_ref]),
        original_oid
    );
    let state_root = state_worktree_for_target_root(&fixture.a).unwrap();
    let goal_a = Path::new("goals/GOALA/goal.json");
    let original_fingerprints = service.load_state_baseline().unwrap().unwrap();
    assert!(matches!(
        service
            .reconstruct_baseline_file(
                &state_root,
                goal_a,
                *original_fingerprints.get(goal_a).unwrap(),
            )
            .unwrap(),
        BaselineFileResolution::Loaded {
            source: BaselineReconstructionSource::RetainedSnapshot,
            ..
        }
    ));

    git(&fixture.a, &["pack-refs", "--all"]);
    assert!(
        !git_common_dir(&fixture.a)
            .unwrap()
            .join(&original_ref)
            .exists()
    );
    service.sync().unwrap();
    assert_eq!(fs::read(&baseline_path).unwrap(), original_bytes);

    write_goal(&fixture.a, "GOALB");
    install_after_baseline_anchor_hook(&fixture.a);
    let interrupted = service.sync().unwrap_err();
    assert!(
        interrupted
            .to_string()
            .contains("retained state-baseline anchor")
    );
    assert_eq!(fs::read(&baseline_path).unwrap(), original_bytes);
    let anchors_after_interruption = git_stdout(
        &fixture.a,
        &[
            "for-each-ref",
            "--format=%(refname)",
            STATE_BASELINE_REF_PREFIX,
        ],
    );
    assert!(anchors_after_interruption.lines().count() >= 2);

    service.sync().unwrap();
    let current: serde_json::Value =
        serde_json::from_slice(&fs::read(&baseline_path).unwrap()).unwrap();
    let current_ref = current["snapshot_ref"].as_str().unwrap();
    assert_ne!(current_ref, original_ref);
    assert!(
        git_stdout(
            &fixture.a,
            &["for-each-ref", "--format=%(refname)", &original_ref]
        )
        .is_empty()
    );

    let fingerprints = current["fingerprints"].clone();
    fs::write(
        &baseline_path,
        serde_json::to_vec_pretty(&fingerprints).unwrap(),
    )
    .unwrap();
    git(&fixture.a, &["update-ref", "-d", current_ref]);
    let loaded = service.load_state_baseline().unwrap().unwrap();
    assert_eq!(
        loaded,
        durable_state_map(&state_root.join(".refine")).unwrap()
    );
    let goal_b = Path::new("goals/GOALB/goal.json");
    assert!(matches!(
        service
            .reconstruct_baseline_file(&state_root, goal_b, *loaded.get(goal_b).unwrap())
            .unwrap(),
        BaselineFileResolution::Loaded {
            source: BaselineReconstructionSource::LegacyHistory,
            ..
        }
    ));
}

#[test]
fn unavailable_baseline_rejects_equal_time_node_disagreement_with_diagnostics() {
    let fixture = SyncFixture::new("node-baseline-unavailable-rejected");
    write_nodes(&fixture.a, &[("node-a", "2026-08-17T08:00:00Z", "unknown")]);
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();
    rewrite_baseline_fingerprint(&fixture.b, "nodes.json", u64::MAX);
    write_nodes(&fixture.a, &[("node-a", "2026-08-17T08:01:00Z", "remote")]);
    fixture.service(&fixture.a).sync().unwrap();
    write_nodes(&fixture.b, &[("node-a", "2026-08-17T08:01:00Z", "local")]);

    let error = fixture.service(&fixture.b).sync().unwrap_err();
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();

    assert!(
        error
            .to_string()
            .contains("recorded baseline bytes were unavailable"),
        "{error}"
    );
    assert_eq!(report.unresolved_paths, vec!["nodes.json"]);
    assert!(report.reconciliation_outcomes.iter().any(|outcome| {
        outcome.path == "nodes.json"
            && outcome.outcome == StateSyncReconciliationKind::BaselineBytesUnavailable
    }));
}

#[test]
fn unrelated_conflict_withholds_a_prepared_node_registry_merge() {
    let fixture = SyncFixture::new("node-merge-atomicity");
    write_nodes(
        &fixture.a,
        &[
            ("node-a", "2026-08-17T08:00:00Z", "unknown"),
            ("node-b", "2026-08-17T08:00:00Z", "unknown"),
        ],
    );
    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    fs::write(refine_a.join("shared.json"), "base\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    write_nodes(
        &fixture.a,
        &[
            ("node-a", "2026-08-17T08:01:00Z", "healthy"),
            ("node-b", "2026-08-17T08:00:00Z", "unknown"),
        ],
    );
    fs::write(refine_a.join("shared.json"), "remote\n").unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    write_nodes(
        &fixture.b,
        &[
            ("node-a", "2026-08-17T08:00:00Z", "unknown"),
            ("node-b", "2026-08-17T08:02:00Z", "healthy"),
        ],
    );
    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    fs::write(refine_b.join("shared.json"), "local\n").unwrap();
    let before = fs::read(refine_b.join("nodes.json")).unwrap();

    let error = fixture.service(&fixture.b).sync().unwrap_err();

    assert!(error.to_string().contains("1 unresolved path"), "{error}");
    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert_eq!(report.unresolved_paths, vec!["shared.json"]);
    assert_eq!(fs::read(refine_b.join("nodes.json")).unwrap(), before);
    assert_eq!(
        git_stdout(
            &state_worktree_for_target_root(&fixture.b).unwrap(),
            &["status", "--short"]
        ),
        ""
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
            .is_some_and(|detail| detail.contains("Merged non-overlapping Goal changes")),
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
fn goal_merge_does_not_use_timestamps_to_hide_competing_lifecycle_changes() {
    let base = br#"{"id":"GOALA","status":"backlog","updated":"2026-08-03T18:20:00Z"}"#;
    let local = br#"{"id":"GOALA","status":"done","updated":"2026-08-03T18:22:00Z"}"#;
    let remote = br#"{"id":"GOALA","status":"todo","updated":"2026-08-03T18:21:00Z"}"#;

    assert!(merge_goal_record(base, local, remote).is_none());
}

#[test]
fn goal_merge_keeps_the_authoritative_start_over_a_concurrent_reassignment_request() {
    let base = valid_goal_record("GOALA", "node-a", "todo", "Base", "2026-08-03T18:20:00Z");
    let local = valid_goal_record(
        "GOALA",
        "node-a",
        "in-progress",
        "Base",
        "2026-08-03T18:21:00Z",
    );
    let remote = valid_goal_record(
        "GOALA",
        "node-b",
        "todo",
        "Clarified",
        "2026-08-03T18:22:00Z",
    );

    let merged = merge_goal_record(&base, &local, &remote).unwrap();
    let merged: serde_json::Value = serde_json::from_slice(&merged).unwrap();
    assert_eq!(merged["status"], "in-progress");
    assert_eq!(merged["node_id"], "node-a");
    assert_eq!(merged["title"], "Clarified");
    assert_eq!(merged["updated"], "2026-08-03T18:22:00Z");
}

#[test]
fn goal_merge_still_rejects_competing_non_start_lifecycle_changes() {
    let base =
        br#"{"id":"GOALA","node_id":"node-a","status":"todo","updated":"2026-08-03T18:20:00Z"}"#;
    let local = br#"{"id":"GOALA","node_id":"node-a","status":"cancelled","updated":"2026-08-03T18:21:00Z"}"#;
    let remote =
        br#"{"id":"GOALA","node_id":"node-b","status":"done","updated":"2026-08-03T18:22:00Z"}"#;

    assert!(merge_goal_record(base, local, remote).is_none());
}

#[test]
fn goal_merge_combines_notes_by_stable_id_and_validates_the_goal_schema() {
    let base = valid_goal_record("GOALA", "node-a", "todo", "Base", "2026-08-03T18:20:00Z");
    let mut local: serde_json::Value = serde_json::from_slice(&base).unwrap();
    let mut remote = local.clone();
    local["notes"] = serde_json::json!([{
        "id": "NOTE-A",
        "author": "A",
        "body": "local",
        "created": "2026-08-03T18:21:00Z",
        "updated": "2026-08-03T18:21:00Z"
    }]);
    local["updated"] = serde_json::json!("2026-08-03T18:21:00Z");
    remote["notes"] = serde_json::json!([{
        "id": "NOTE-B",
        "author": "B",
        "body": "remote",
        "created": "2026-08-03T18:22:00Z",
        "updated": "2026-08-03T18:22:00Z"
    }]);
    remote["updated"] = serde_json::json!("2026-08-03T18:22:00Z");
    let local = serde_json::to_vec(&local).unwrap();
    let remote = serde_json::to_vec(&remote).unwrap();

    let merged = merge_goal_record(&base, &local, &remote).unwrap();
    let goal: crate::model::goal::Goal = serde_json::from_slice(&merged).unwrap();
    assert_eq!(
        goal.notes
            .iter()
            .map(|note| note.id.as_str())
            .collect::<Vec<_>>(),
        vec!["NOTE-A", "NOTE-B"]
    );
}

#[test]
fn goal_merge_keeps_rounds_atomic_across_workflow_authority_changes() {
    let base = valid_goal_record("GOALA", "node-a", "todo", "Base", "2026-08-03T18:20:00Z");
    let mut local: serde_json::Value = serde_json::from_slice(&base).unwrap();
    let mut remote = local.clone();
    local["rounds"] = serde_json::json!([{
        "reporter": "A",
        "prompt": "Implement",
        "created": "2026-08-03T18:21:00Z",
        "updated": "2026-08-03T18:21:00Z",
        "guidance_decision": null,
        "governance": null,
        "quality": null,
        "logs": []
    }]);
    remote["status"] = serde_json::json!("plan");
    assert!(
        merge_goal_record(
            &base,
            &serde_json::to_vec(&local).unwrap(),
            &serde_json::to_vec(&remote).unwrap(),
        )
        .is_none()
    );
}

#[test]
fn goal_merge_rejects_cross_field_invalid_round_evidence() {
    let base = valid_goal_record("GOALA", "node-a", "todo", "Base", "2026-08-03T18:20:00Z");
    let mut local: serde_json::Value = serde_json::from_slice(&base).unwrap();
    let mut remote = local.clone();
    local["rounds"] = serde_json::json!([{
        "reporter": "A",
        "prompt": "Implement",
        "created": "2026-08-03T18:21:00Z",
        "updated": "2026-08-03T18:21:00Z",
        "guidance_decision": null,
        "implementation_report": "completed without its timestamp",
        "governance": null,
        "quality": null,
        "logs": []
    }]);
    remote["notes"] = serde_json::json!([{
        "id": "NOTE-A",
        "author": "A",
        "body": "remote",
        "created": "2026-08-03T18:22:00Z",
        "updated": "2026-08-03T18:22:00Z"
    }]);

    assert!(
        merge_goal_record(
            &base,
            &serde_json::to_vec(&local).unwrap(),
            &serde_json::to_vec(&remote).unwrap(),
        )
        .is_none()
    );
}

#[test]
fn unresolved_goal_conflict_does_not_write_other_prepared_merges() {
    let fixture = SyncFixture::new("mixed-goal-conflicts");
    for id in ["GOALA", "GOALB"] {
        write_goal(&fixture.a, id);
        fs::write(
            refine_dir_for_target_root(&fixture.a)
                .unwrap()
                .join(format!("goals/{id}/goal.json")),
            valid_goal_record(id, "node-a", "backlog", "Base", "2026-08-03T18:20:00Z"),
        )
        .unwrap();
    }
    fixture.service(&fixture.a).sync().unwrap();
    fixture.service(&fixture.b).sync().unwrap();

    let refine_a = refine_dir_for_target_root(&fixture.a).unwrap();
    fs::write(
        refine_a.join("goals/GOALA/goal.json"),
        valid_goal_record("GOALA", "node-b", "backlog", "Base", "2026-08-03T18:21:00Z"),
    )
    .unwrap();
    fs::write(
        refine_a.join("goals/GOALB/goal.json"),
        valid_goal_record("GOALB", "node-a", "todo", "Base", "2026-08-03T18:21:00Z"),
    )
    .unwrap();
    fixture.service(&fixture.a).sync().unwrap();

    let refine_b = refine_dir_for_target_root(&fixture.b).unwrap();
    for id in ["GOALA", "GOALB"] {
        fs::write(
            refine_b.join(format!("goals/{id}/goal.json")),
            valid_goal_record(id, "node-b", "done", "Base", "2026-08-03T18:22:00Z"),
        )
        .unwrap();
    }

    let _error = fixture.service(&fixture.b).sync().unwrap_err();

    let report = latest_state_sync_conflict_report(&fixture.b.join("run"))
        .unwrap()
        .unwrap();
    assert!(
        report
            .unresolved_paths
            .contains(&"goals/GOALB/goal.json".to_string())
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
