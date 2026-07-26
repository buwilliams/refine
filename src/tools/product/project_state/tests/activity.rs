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
