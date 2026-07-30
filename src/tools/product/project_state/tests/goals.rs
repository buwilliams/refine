use super::*;

#[test]
fn projection_query_counts_goal_statuses() {
    let mut goals = BTreeMap::new();
    goals.insert(
        "goal-1".to_string(),
        goal_projection("goal-1", GoalStatus::Todo, Some("node-a")),
    );
    goals.insert(
        "goal-2".to_string(),
        goal_projection("goal-2", GoalStatus::Todo, Some("node-a")),
    );
    goals.insert(
        "goal-3".to_string(),
        goal_projection("goal-3", GoalStatus::Done, None),
    );

    let snapshot = ProjectionSnapshot {
        refine_dir: None,
        version: PROJECTION_SNAPSHOT_VERSION,
        generated_at: "now".to_string(),
        source_fingerprints: BTreeMap::new(),
        goals,
        features: BTreeMap::new(),
        activity: BTreeMap::new(),
        changes: BTreeMap::new(),
        dashboard: DashboardProjection::default(),
        runtime: RuntimeProjection::default(),
    };

    let counts = snapshot.status_counts();
    assert_eq!(counts.get(&GoalStatus::Todo), Some(&2));
    assert_eq!(counts.get(&GoalStatus::Done), Some(&1));

    let index = ProjectionIndex::build(&snapshot);
    assert_eq!(index.goals_by_node["node-a"].len(), 2);
    assert!(index.standalone_goal_ids.contains("goal-3"));
}

#[test]
fn file_store_persists_and_loads_projection_snapshot() {
    let temp_root = unique_temp_dir("projection-store");
    let refine_dir = temp_root.join("refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    let store = FileProjectStateStore::new(&refine_dir);
    store.initialize().unwrap();

    let mut goals = BTreeMap::new();
    goals.insert(
        "goal-1".to_string(),
        goal_projection("goal-1", GoalStatus::Todo, Some("node-a")),
    );
    let snapshot = ProjectionSnapshot {
        refine_dir: None,
        version: PROJECTION_SNAPSHOT_VERSION,
        generated_at: "now".to_string(),
        source_fingerprints: BTreeMap::new(),
        goals,
        features: BTreeMap::new(),
        activity: BTreeMap::new(),
        changes: BTreeMap::new(),
        dashboard: DashboardProjection::default(),
        runtime: RuntimeProjection::default(),
    };

    store
        .persist_projection_snapshot(&cache_dir, &snapshot)
        .unwrap();
    let loaded = store.load_projection_snapshot(&cache_dir).unwrap().unwrap();
    assert_eq!(loaded.goals.len(), 1);
    assert_eq!(loaded.version, PROJECTION_SNAPSHOT_VERSION);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_store_persists_projection_snapshot_concurrently() {
    let temp_root = unique_temp_dir("projection-store-concurrent");
    let refine_dir = temp_root.join("refine");
    let cache_dir = temp_root.join("run").join("cache");
    let store = FileProjectStateStore::new(&refine_dir);
    store.initialize().unwrap();

    let barrier = Arc::new(Barrier::new(12));
    let handles = (0..12)
        .map(|index| {
            let store = store.clone();
            let cache_dir = cache_dir.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let snapshot = ProjectionSnapshot {
                    generated_at: format!("concurrent-{index}"),
                    ..ProjectionSnapshot::default()
                };
                barrier.wait();
                store.persist_projection_snapshot(&cache_dir, &snapshot)
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    assert!(
        store
            .load_projection_snapshot(&cache_dir)
            .unwrap()
            .is_some()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_store_ignores_incompatible_snapshot_versions() {
    let temp_root = unique_temp_dir("projection-version");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    let store = FileProjectStateStore::new(temp_root.join("refine"));
    let mut snapshot = store.rebuild_projection().unwrap();
    snapshot.version = PROJECTION_SNAPSHOT_VERSION + 1;

    store
        .persist_projection_snapshot(&cache_dir, &snapshot)
        .unwrap();
    assert!(
        store
            .load_projection_snapshot(&cache_dir)
            .unwrap()
            .is_none()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn rebuild_projection_with_runtime_root_avoids_refine_runtime_processes() {
    let temp_root = unique_temp_dir("projection-runtime-root");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "one\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();

    FileProjectStateStore::with_runtime_root(&refine_dir, &runtime_root)
        .rebuild_projection()
        .unwrap();

    assert!(!refine_dir.join("runtime/processes").exists());
    assert!(!runtime_root.join("processes").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

// Notes and Round prompts have no upper bound and accumulate for as long as a
// Goal is worked, so carrying them in full made searchable text the largest
// per-Goal cost in the projection. Only a prefix stays resident now — but
// nothing may become unfindable as a result, so a term past the prefix has to
// be reached by consulting the record.
#[test]
fn text_past_the_resident_prefix_is_still_searchable() {
    use crate::tools::product::project_state::helpers::RESIDENT_SEARCH_TEXT_LIMIT;

    let temp_root = unique_temp_dir("projection-deep-search");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("GO").join("AL1");
    fs::create_dir_all(&goal_dir).unwrap();
    // A distinctive term buried well past the resident budget, as a long Round
    // history would bury it.
    let filler = "routine implementation detail ".repeat(200);
    fs::write(
        goal_dir.join("goal.json"),
        format!(
            r#"{{"id":"GOAL1","name":"Wordy Goal","status":"todo",
                 "rounds":[{{"reporter":"Buddy","prompt":"{filler} sarsaparilla"}}],
                 "notes":[]}}"#
        ),
    )
    .unwrap();

    let store = FileProjectStateStore::new(&refine_dir);
    let snapshot = store.rebuild_projection().unwrap();

    let goal = &snapshot.goals["GOAL1"];
    assert!(
        goal.searchable_text.len() <= RESIDENT_SEARCH_TEXT_LIMIT,
        "resident text must stay within its budget, was {}",
        goal.searchable_text.len()
    );
    assert!(
        !goal.searchable_text.contains("sarsaparilla"),
        "the fixture must actually bury the term past the prefix"
    );

    // Found anyway, by reading the record the prefix came from.
    let found = snapshot.list_goals(GoalProjectionQuery {
        q: Some("sarsaparilla".to_string()),
        page: PageRequest::default(),
        ..GoalProjectionQuery::default()
    });
    assert_eq!(found.total, 1, "buried text must remain searchable");
    assert_eq!(found.goals[0].id, "GOAL1");

    // And a term that is genuinely absent still does not match, so falling
    // through to the record cannot manufacture a hit.
    let missing = snapshot.list_goals(GoalProjectionQuery {
        q: Some("bergamot".to_string()),
        page: PageRequest::default(),
        ..GoalProjectionQuery::default()
    });
    assert_eq!(missing.total, 0);

    fs::remove_dir_all(temp_root).unwrap();
}
