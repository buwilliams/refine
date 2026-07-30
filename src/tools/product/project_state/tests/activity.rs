use super::*;

#[test]
fn rebuild_projection_scans_git_changes_and_joins_goal_display_fields() {
    let temp_root = unique_temp_dir("projection-changes");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("GO").join("AL1");
    fs::create_dir_all(&goal_dir).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "one\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Change-linked Goal",
              "status": "done",
              "priority": "high",
              "branch_name": "main",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();
    fs::write(temp_root.join("app.txt"), "unrelated\n").unwrap();
    git(&temp_root, &["commit", "-am", "maintenance update"]).unwrap();
    fs::write(temp_root.join("app.txt"), "two\n").unwrap();
    git(&temp_root, &["commit", "-am", "GOAL1 update app"]).unwrap();

    let snapshot = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    assert!(snapshot.source_fingerprints.contains_key("git:HEAD"));
    let all_changes = snapshot.list_changes(ChangeProjectionQuery {
        page: PageRequest::default(),
        ..ChangeProjectionQuery::default()
    });
    assert_eq!(all_changes.total, 1);
    assert_eq!(all_changes.changes[0].subject, "GOAL1 update app");
    assert_eq!(all_changes.changes[0].goal_id.as_deref(), Some("GOAL1"));
    let changes = snapshot.list_changes(ChangeProjectionQuery {
        q: Some("GOAL1 update".to_string()),
        goal_id: Some("GOAL1".to_string()),
        status: Some(GoalStatus::Done),
        priority: Some("high".to_string()),
        page: PageRequest::default(),
        ..ChangeProjectionQuery::default()
    });
    assert_eq!(changes.total, 1);
    assert_eq!(changes.changes[0].goal_id.as_deref(), Some("GOAL1"));
    assert_eq!(
        changes.changes[0].goal_name.as_deref(),
        Some("Change-linked Goal")
    );
    assert_eq!(changes.changes[0].goal_status, Some(GoalStatus::Done));
    assert_eq!(changes.changes[0].goal_priority.as_deref(), Some("high"));

    fs::remove_dir_all(temp_root).unwrap();
}

// Round-log activity used to be materialized in full: every line every Goal had
// ever logged became a resident projection entry with its own searchable text.
// Log volume tracks retries and failures rather than Goal count, so resident
// memory grew with how badly the fleet was doing — the deployment shape that
// Goal count alone does not explain.
#[test]
fn projected_round_log_activity_keeps_a_bounded_newest_window() {
    use crate::tools::product::project_state::store::PROJECTED_ACTIVITY_PER_GOAL_LIMIT;

    let temp_root = unique_temp_dir("projection-activity-bound");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("GO").join("AL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Noisy Goal",
              "status": "in-progress",
              "priority": "high",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();

    // Far more than one Goal may contribute, as a retry loop would produce.
    let noisy_line_count = PROJECTED_ACTIVITY_PER_GOAL_LIMIT * 20;
    let mut sidecar = String::new();
    for index in 0..noisy_line_count {
        sidecar.push_str(&format!(
            "{{\"round_idx\":1,\"datetime\":\"2026-01-01T{:02}:{:02}:{:02}Z\",\"severity\":\"info\",\"category\":\"agent\",\"message\":\"line {index}\"}}\n",
            index / 3600,
            (index / 60) % 60,
            index % 60
        ));
    }
    fs::write(goal_dir.join("logs.jsonl"), sidecar).unwrap();

    let snapshot = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();

    let round_log_ids = snapshot
        .activity
        .keys()
        .filter(|id| id.starts_with("round-log:"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        round_log_ids.len(),
        PROJECTED_ACTIVITY_PER_GOAL_LIMIT,
        "one Goal must not contribute more than its window"
    );

    // The window keeps the newest entries, and identity stays tied to absolute
    // position in the sidecar so it survives the file growing.
    let newest_position = noisy_line_count - 1;
    let oldest_kept_position = noisy_line_count - PROJECTED_ACTIVITY_PER_GOAL_LIMIT;
    assert!(
        round_log_ids.contains(&format!("round-log:GOAL1:1:{newest_position}")),
        "newest entry must survive: {round_log_ids:?}"
    );
    assert!(
        round_log_ids.contains(&format!("round-log:GOAL1:1:{oldest_kept_position}")),
        "window must reach back exactly its limit"
    );
    assert!(
        !round_log_ids.contains(&format!("round-log:GOAL1:1:{}", oldest_kept_position - 1)),
        "entries older than the window must be dropped"
    );

    fs::remove_dir_all(temp_root).unwrap();
}
