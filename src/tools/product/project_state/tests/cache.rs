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

#[test]
fn file_store_rebuilds_legacy_snapshot_before_deserializing_current_schema() {
    let temp_root = unique_temp_dir("projection-legacy-schema");
    let refine_dir = temp_root.join(".refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    let goal_dir = refine_dir.join("goals").join("GO").join("AL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Rebuilt from source",
              "status": "todo",
              "rounds": []
            }"#,
    )
    .unwrap();
    fs::write(
        cache_dir.join(PROJECTION_SNAPSHOT_FILE),
        r#"{
              "version": 1,
              "generated_at": "legacy",
              "source_fingerprints": {},
              "gaps": {}
            }"#,
    )
    .unwrap();

    let store = FileProjectStateStore::new(&refine_dir);
    let rebuilt = store.load_or_refresh_projection(&cache_dir).unwrap();

    assert_eq!(rebuilt.version, PROJECTION_SNAPSHOT_VERSION);
    assert_eq!(rebuilt.goals["GOAL1"].goal.name, "Rebuilt from source");
    let persisted: serde_json::Value =
        serde_json::from_slice(&fs::read(cache_dir.join(PROJECTION_SNAPSHOT_FILE)).unwrap())
            .unwrap();
    assert_eq!(
        persisted["version"].as_u64(),
        Some(PROJECTION_SNAPSHOT_VERSION)
    );
    assert!(persisted.get("goals").is_some());
    assert!(persisted.get("gaps").is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_store_rebuilds_malformed_projection_snapshot() {
    let temp_root = unique_temp_dir("projection-malformed");
    let refine_dir = temp_root.join(".refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&cache_dir).unwrap();
    fs::write(
        cache_dir.join(PROJECTION_SNAPSHOT_FILE),
        br#"{"version":2,"goals":"#,
    )
    .unwrap();

    let store = FileProjectStateStore::new(&refine_dir);
    let rebuilt = store.load_or_refresh_projection(&cache_dir).unwrap();

    assert_eq!(rebuilt.version, PROJECTION_SNAPSHOT_VERSION);
    assert!(rebuilt.goals.is_empty());
    assert!(
        store
            .load_projection_snapshot(&cache_dir)
            .unwrap()
            .is_some()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

// Staleness used to be a single boolean, so any change discarded the cached
// projection and re-read every Goal and Feature record. A Goal write is the
// most frequent event in a running fleet, which made the full-corpus path the
// normal path and tied the cost of every mutation to total project size.
#[test]
fn goal_writes_patch_the_projection_without_a_full_rebuild() {
    let temp_root = unique_temp_dir("projection-incremental");
    let refine_dir = temp_root.join(".refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    for (shard, rest, id) in [("GO", "AL1", "GOAL1"), ("GO", "AL2", "GOAL2")] {
        let goal_dir = refine_dir.join("goals").join(shard).join(rest);
        fs::create_dir_all(&goal_dir).unwrap();
        fs::write(
            goal_dir.join("goal.json"),
            format!(
                r#"{{"id":"{id}","name":"{id} original","status":"todo","rounds":[]}}"#
            ),
        )
        .unwrap();
    }

    let store = FileProjectStateStore::new(&refine_dir);
    // Cold cache legitimately rebuilds once.
    store.load_or_refresh_projection(&cache_dir).unwrap();
    FileProjectStateStore::reset_rebuild_count(&refine_dir);

    let goal_path = refine_dir.join("goals").join("GO").join("AL1");
    fs::write(
        goal_path.join("goal.json"),
        r#"{"id":"GOAL1","name":"GOAL1 renamed","status":"done","rounds":[]}"#,
    )
    .unwrap();

    let patched = store.load_or_refresh_projection(&cache_dir).unwrap();

    assert_eq!(
        FileProjectStateStore::rebuild_count(&refine_dir),
        0,
        "a Goal write must not re-read the whole corpus"
    );
    assert_eq!(patched.goals["GOAL1"].goal.name, "GOAL1 renamed");
    assert_eq!(patched.goals["GOAL1"].goal.status, GoalStatus::Done);
    // The untouched Goal survives, and derived aggregates reflect both.
    assert_eq!(patched.goals["GOAL2"].goal.name, "GOAL2 original");
    assert_eq!(
        patched.dashboard.all_node_status_counts.get(&GoalStatus::Done),
        Some(&1)
    );
    assert_eq!(
        patched.dashboard.all_node_status_counts.get(&GoalStatus::Todo),
        Some(&1)
    );

    // The patched projection is what a later read observes, without rebuilding.
    let reloaded = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(FileProjectStateStore::rebuild_count(&refine_dir), 0);
    assert_eq!(reloaded.goals["GOAL1"].goal.name, "GOAL1 renamed");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn removing_a_goal_record_drops_it_from_the_patched_projection() {
    let temp_root = unique_temp_dir("projection-incremental-remove");
    let refine_dir = temp_root.join(".refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    for (rest, id) in [("AL1", "GOAL1"), ("AL2", "GOAL2")] {
        let goal_dir = refine_dir.join("goals").join("GO").join(rest);
        fs::create_dir_all(&goal_dir).unwrap();
        fs::write(
            goal_dir.join("goal.json"),
            format!(r#"{{"id":"{id}","name":"{id}","status":"todo","rounds":[]}}"#),
        )
        .unwrap();
    }

    let store = FileProjectStateStore::new(&refine_dir);
    store.load_or_refresh_projection(&cache_dir).unwrap();
    FileProjectStateStore::reset_rebuild_count(&refine_dir);

    fs::remove_dir_all(refine_dir.join("goals").join("GO").join("AL2")).unwrap();
    let patched = store.load_or_refresh_projection(&cache_dir).unwrap();

    assert_eq!(FileProjectStateStore::rebuild_count(&refine_dir), 0);
    assert!(patched.goals.contains_key("GOAL1"));
    assert!(
        !patched.goals.contains_key("GOAL2"),
        "a deleted record must leave the projection"
    );
    assert_eq!(
        patched.dashboard.all_node_status_counts.get(&GoalStatus::Todo),
        Some(&1)
    );

    fs::remove_dir_all(temp_root).unwrap();
}
