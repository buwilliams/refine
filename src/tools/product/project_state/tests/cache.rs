use super::*;

#[test]
fn file_store_loads_cached_projection_until_fingerprints_change() {
    let temp_root = unique_temp_dir("projection-refresh");
    let refine_dir = temp_root.join(".refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    let goal_dir = refine_dir.join("goals").join("GO").join("AL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Cached name",
              "status": "todo",
              "rounds": []
            }"#,
    )
    .unwrap();
    let store = FileProjectStateStore::new(&refine_dir);
    let mut snapshot = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(snapshot.goals["GOAL1"].goal.name, "Cached name");

    snapshot.generated_at = "cached-sentinel".to_string();
    store
        .persist_projection_snapshot(&cache_dir, &snapshot)
        .unwrap();
    let cached = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(cached.generated_at, "cached-sentinel");

    FileLogService::new(&refine_dir)
        .append_round_log(
            "GOAL1",
            0,
            LogEntry {
                datetime: "2026-01-03T00:00:00Z".to_string(),
                severity: "info".to_string(),
                category: "workflow".to_string(),
                message: "Sidecar cache refresh".to_string(),
                details: None,
                actions: Vec::new(),
                actor: Some("workflow".to_string()),
                goal_id: Some("GOAL1".to_string()),
            },
        )
        .unwrap();
    let sidecar_refreshed = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_ne!(sidecar_refreshed.generated_at, "cached-sentinel");
    assert_eq!(
        sidecar_refreshed
            .list_activity(ActivityProjectionQuery {
                goal_id: Some("GOAL1".to_string()),
                ..ActivityProjectionQuery::default()
            })
            .activity[0]
            .message,
        "Sidecar cache refresh"
    );

    let mut snapshot = sidecar_refreshed;
    snapshot.generated_at = "cached-after-sidecar".to_string();
    store
        .persist_projection_snapshot(&cache_dir, &snapshot)
        .unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Refreshed name with changed refine content",
              "status": "todo",
              "rounds": []
            }"#,
    )
    .unwrap();
    let refreshed = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(
        refreshed.goals["GOAL1"].goal.name,
        "Refreshed name with changed refine content"
    );
    assert_ne!(refreshed.generated_at, "cached-after-sidecar");

    fs::remove_dir_all(temp_root).unwrap();
}
