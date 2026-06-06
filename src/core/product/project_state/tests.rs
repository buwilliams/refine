use crate::core::observability::activity::ACTIVITY_LOG_FILE;
use crate::model::feature::FeatureRollup;
use crate::model::workflow::GapStatus;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::feature::FeatureIndexProjection;
use crate::model::gap::{GapIndexProjection, GapPriority};
use crate::model::log::ActivityEntry;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn projection_query_counts_gap_statuses() {
    let mut gaps = BTreeMap::new();
    gaps.insert(
        "gap-1".to_string(),
        gap_projection("gap-1", GapStatus::Todo, Some("node-a")),
    );
    gaps.insert(
        "gap-2".to_string(),
        gap_projection("gap-2", GapStatus::Todo, Some("node-a")),
    );
    gaps.insert(
        "gap-3".to_string(),
        gap_projection("gap-3", GapStatus::Done, None),
    );

    let snapshot = ProjectionSnapshot {
        version: PROJECTION_SNAPSHOT_VERSION,
        generated_at: "now".to_string(),
        source_fingerprints: BTreeMap::new(),
        gaps,
        features: BTreeMap::new(),
        activity: BTreeMap::new(),
        changes: BTreeMap::new(),
        dashboard: DashboardProjection::default(),
        runtime: RuntimeProjection::default(),
    };

    let counts = snapshot.status_counts();
    assert_eq!(counts.get(&GapStatus::Todo), Some(&2));
    assert_eq!(counts.get(&GapStatus::Done), Some(&1));

    let index = ProjectionIndex::build(&snapshot);
    assert_eq!(index.gaps_by_node["node-a"].len(), 2);
    assert!(index.standalone_gap_ids.contains("gap-3"));
}

#[test]
fn projection_query_filters_sorts_and_pages_gaps_and_features() {
    let mut gap_one = gap_projection("gap-1", GapStatus::Todo, Some("default"));
    gap_one.gap.name = "OAuth callback broken".to_string();
    gap_one.gap.reporter = Some("Alice".to_string());
    gap_one.gap.round_count = 2;
    gap_one.gap.feature_id = Some("feature-1".to_string());
    gap_one.gap.priority = GapPriority::High;
    gap_one.searchable_text = "OAuth callback broken login notes".to_string();
    gap_one.activity_ids = vec!["act-1".to_string()];

    let mut gap_two = gap_projection("gap-2", GapStatus::Done, Some("node-b"));
    gap_two.gap.name = "Settings polish".to_string();
    gap_two.gap.reporter = Some("Bob".to_string());
    gap_two.gap.round_count = 1;

    let mut gaps = BTreeMap::new();
    gaps.insert(gap_one.gap.id.clone(), gap_one);
    gaps.insert(gap_two.gap.id.clone(), gap_two);

    let mut activity = BTreeMap::new();
    activity.insert(
        "act-1".to_string(),
        ActivitySummaryProjection {
            entry: ActivityEntry {
                id: "act-1".to_string(),
                datetime: "2026-01-01T00:00:00Z".to_string(),
                severity: "error".to_string(),
                category: "quality".to_string(),
                message: "OAuth failed".to_string(),
                gap_id: Some("gap-1".to_string()),
                actor: Some("browser".to_string()),
                details: None,
                actions: Vec::new(),
            },
            searchable_text: "OAuth failed".to_string(),
        },
    );

    let feature = FeatureSummaryProjection {
        feature: FeatureIndexProjection {
            id: "feature-1".to_string(),
            name: "Auth work".to_string(),
            description: Some("OAuth fixes".to_string()),
            reporter: Some("Alice".to_string()),
            node_id: Some("default".to_string()),
            created: "created".to_string(),
            updated: "updated".to_string(),
            json_path: "feature.json".to_string(),
        },
        status: GapStatus::Todo,
        gap_ids: vec!["gap-1".to_string()],
        rollup: FeatureRollup {
            status: GapStatus::Todo,
            gap_count: 1,
            done_count: 0,
            active_count: 0,
            failed_count: 0,
            cancelled_count: 0,
            blocked_count: 0,
            next_gap: Some("gap-1".to_string()),
        },
    };
    let mut features = BTreeMap::new();
    features.insert("feature-1".to_string(), feature);

    let snapshot = ProjectionSnapshot {
        version: PROJECTION_SNAPSHOT_VERSION,
        generated_at: "now".to_string(),
        source_fingerprints: BTreeMap::new(),
        gaps,
        features,
        activity,
        changes: BTreeMap::new(),
        dashboard: DashboardProjection::default(),
        runtime: RuntimeProjection::default(),
    };

    let gaps = snapshot.list_gaps(GapProjectionQuery {
        q: Some("oauth".to_string()),
        feature: Some("feature-1".to_string()),
        severity: Some("error".to_string()),
        category: Some("quality".to_string()),
        actor: Some("browser".to_string()),
        rounds_gte: Some(2),
        page: PageRequest {
            sort: "priority".to_string(),
            dir: "desc".to_string(),
            ..PageRequest::default()
        },
        ..GapProjectionQuery::default()
    });
    assert_eq!(gaps.total, 1);
    assert_eq!(gaps.gaps[0].id, "gap-1");
    assert_eq!(gaps.filtered_status_counts.get(&GapStatus::Todo), Some(&1));
    assert_eq!(gaps.matching_ids, vec!["gap-1"]);

    let activity = snapshot.list_activity(ActivityProjectionQuery {
        q: Some("oauth".to_string()),
        severity: Some("error".to_string()),
        category: Some("quality".to_string()),
        actor: Some("browser".to_string()),
        page: PageRequest {
            sort: "message".to_string(),
            dir: "asc".to_string(),
            ..PageRequest::default()
        },
        ..ActivityProjectionQuery::default()
    });
    assert_eq!(activity.total, 1);
    assert_eq!(activity.activity[0].id, "act-1");
    assert_eq!(activity.matching_ids, vec!["act-1"]);
    assert_eq!(activity.facets.categories, vec!["quality"]);
    assert_eq!(activity.facets.severities, vec!["error"]);
    assert_eq!(activity.facets.actors, vec!["browser"]);

    let features = snapshot.list_features(FeatureProjectionQuery {
        q: Some("oauth".to_string()),
        reporter: Some("Alice".to_string()),
        status: Some(GapStatus::Todo),
        node: Some("current".to_string()),
        current_node_id: Some("default".to_string()),
        page: PageRequest::default(),
    });
    assert_eq!(features.total, 1);
    assert_eq!(features.features[0].feature.id, "feature-1");
}

#[test]
fn file_store_persists_and_loads_projection_snapshot() {
    let temp_root = unique_temp_dir("projection-store");
    let durable_root = temp_root.join("durable");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    let store = FileProjectStateStore::new(&durable_root);
    store.initialize().unwrap();

    let mut gaps = BTreeMap::new();
    gaps.insert(
        "gap-1".to_string(),
        gap_projection("gap-1", GapStatus::Todo, Some("node-a")),
    );
    let snapshot = ProjectionSnapshot {
        version: PROJECTION_SNAPSHOT_VERSION,
        generated_at: "now".to_string(),
        source_fingerprints: BTreeMap::new(),
        gaps,
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
    assert_eq!(loaded.gaps.len(), 1);
    assert_eq!(loaded.version, PROJECTION_SNAPSHOT_VERSION);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_store_persists_projection_snapshot_concurrently() {
    let temp_root = unique_temp_dir("projection-store-concurrent");
    let durable_root = temp_root.join("durable");
    let cache_dir = temp_root.join("run").join("cache");
    let store = FileProjectStateStore::new(&durable_root);
    store.initialize().unwrap();

    let barrier = Arc::new(Barrier::new(12));
    let handles = (0..12)
        .map(|index| {
            let store = store.clone();
            let cache_dir = cache_dir.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut snapshot = ProjectionSnapshot::default();
                snapshot.generated_at = format!("concurrent-{index}");
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
    let store = FileProjectStateStore::new(temp_root.join("durable"));
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
fn file_store_loads_cached_projection_until_fingerprints_change() {
    let temp_root = unique_temp_dir("projection-refresh");
    let durable_root = temp_root.join(".refine");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    let gap_dir = durable_root.join("gaps").join("GA").join("P1");
    fs::create_dir_all(&gap_dir).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "Cached name",
              "status": "todo",
              "rounds": []
            }"#,
    )
    .unwrap();
    let store = FileProjectStateStore::new(&durable_root);
    let mut snapshot = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(snapshot.gaps["GAP1"].gap.name, "Cached name");

    snapshot.generated_at = "cached-sentinel".to_string();
    store
        .persist_projection_snapshot(&cache_dir, &snapshot)
        .unwrap();
    let cached = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(cached.generated_at, "cached-sentinel");

    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "Refreshed name with changed durable content",
              "status": "todo",
              "rounds": []
            }"#,
    )
    .unwrap();
    let refreshed = store.load_or_refresh_projection(&cache_dir).unwrap();
    assert_eq!(
        refreshed.gaps["GAP1"].gap.name,
        "Refreshed name with changed durable content"
    );
    assert_ne!(refreshed.generated_at, "cached-sentinel");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn rebuild_projection_scans_python_style_gap_and_feature_records() {
    let temp_root = unique_temp_dir("projection-rebuild");
    let durable_root = temp_root.join(".refine");
    let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
    let remote_gap_dir = durable_root.join("gaps").join("02").join("GAP2");
    let feature_dir = durable_root.join("features").join("01").join("FEATURE1");
    fs::create_dir_all(&gap_dir).unwrap();
    fs::create_dir_all(&remote_gap_dir).unwrap();
    fs::create_dir_all(&feature_dir).unwrap();
    fs::create_dir_all(durable_root.join("logs")).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "Fix login",
              "status": "todo",
              "priority": "high",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "feature_id": "FEATURE1",
              "feature_order": 2,
              "rounds": [
                {"reporter": "Buddy", "actual": "Broken", "target": "Works"}
              ],
              "notes": [{"body": "OAuth path"}]
            }"#,
    )
    .unwrap();
    fs::write(
        remote_gap_dir.join("gap.json"),
        r#"{
              "id": "GAP2",
              "name": "Remote failure",
              "status": "failed",
              "priority": "medium",
              "node_id": "node-b",
              "rounds": [{"reporter": "Remote", "actual": "Broken", "target": "Fixed"}],
              "notes": []
            }"#,
    )
    .unwrap();
    fs::write(
        feature_dir.join("feature.json"),
        r#"{
              "id": "FEATURE1",
              "name": "Authentication",
              "description": "Login work",
              "reporter": "Buddy",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z"
            }"#,
    )
    .unwrap();
    fs::write(
            durable_root.join(ACTIVITY_LOG_FILE),
            concat!(
                "{\"id\":\"act-1\",\"datetime\":\"2026-01-03T00:00:00Z\",\"severity\":\"error\",\"category\":\"quality\",\"message\":\"Remote QA failed\",\"gap_id\":\"GAP2\",\"actor\":\"browser\",\"details\":{\"selector\":\"#app\"},\"actions\":[]}\n",
                "{\"id\":\"act-2\",\"datetime\":\"2026-01-04T00:00:00Z\",\"severity\":\"info\",\"category\":\"state\",\"message\":\"Feature changed\",\"gap_id\":null,\"actor\":\"system\",\"details\":null,\"actions\":[]}\n"
            ),
        )
        .unwrap();

    let snapshot = FileProjectStateStore::new(&durable_root)
        .rebuild_projection()
        .unwrap();
    let gap = &snapshot.gaps["GAP1"];
    assert_eq!(gap.gap.status, GapStatus::Todo);
    assert_eq!(gap.gap.priority, GapPriority::High);
    assert_eq!(gap.gap.reporter.as_deref(), Some("Buddy"));
    assert_eq!(gap.gap.round_count, 1);
    assert_eq!(gap.gap.node_id.as_deref(), Some("default"));
    assert!(gap.searchable_text.contains("OAuth path"));

    let feature = &snapshot.features["FEATURE1"];
    assert_eq!(feature.gap_ids, vec!["GAP1"]);
    assert_eq!(feature.rollup.gap_count, 1);
    assert_eq!(feature.rollup.next_gap.as_deref(), Some("GAP1"));
    assert!(
        snapshot
            .source_fingerprints
            .contains_key("gaps/01/GAP1/gap.json")
    );
    assert!(
        snapshot.source_fingerprints["gaps/01/GAP1/gap.json"]
            .content_hash
            .is_some()
    );
    assert_eq!(
        snapshot
            .dashboard
            .all_node_status_counts
            .get(&GapStatus::Todo),
        Some(&1)
    );
    assert_eq!(
        snapshot
            .dashboard
            .all_node_status_counts
            .get(&GapStatus::Failed),
        Some(&1)
    );
    assert_eq!(
        snapshot
            .dashboard
            .current_node_status_counts
            .get(&GapStatus::Todo),
        Some(&1)
    );
    assert_eq!(
        snapshot
            .dashboard
            .current_node_status_counts
            .get(&GapStatus::Failed),
        None
    );
    assert_eq!(snapshot.dashboard.attention_indicators.len(), 1);
    assert_eq!(snapshot.activity.len(), 2);
    assert_eq!(snapshot.gaps["GAP2"].activity_ids, vec!["act-1"]);
    assert!(snapshot.activity["act-1"].searchable_text.contains("#app"));
    assert_eq!(
        snapshot.dashboard.recent_activity_ids,
        vec!["act-2".to_string(), "act-1".to_string()]
    );
    assert!(
        snapshot
            .source_fingerprints
            .contains_key("logs/activity.jsonl")
    );
    let activity_filtered = snapshot.list_gaps(GapProjectionQuery {
        severity: Some("error".to_string()),
        category: Some("quality".to_string()),
        actor: Some("browser".to_string()),
        ..GapProjectionQuery::default()
    });
    assert_eq!(activity_filtered.matching_ids, vec!["GAP2"]);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn rebuild_projection_scans_git_changes_and_joins_gap_display_fields() {
    let temp_root = unique_temp_dir("projection-changes");
    let durable_root = temp_root.join(".refine");
    let gap_dir = durable_root.join("gaps").join("GA").join("P1");
    fs::create_dir_all(&gap_dir).unwrap();
    git(&temp_root, &["init"]).unwrap();
    git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
    git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
    fs::write(temp_root.join("app.txt"), "one\n").unwrap();
    git(&temp_root, &["add", "app.txt"]).unwrap();
    git(&temp_root, &["commit", "-m", "initial"]).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "Change-linked Gap",
              "status": "done",
              "priority": "high",
              "branch_name": "main",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();
    fs::write(temp_root.join("app.txt"), "two\n").unwrap();
    git(&temp_root, &["commit", "-am", "GAP1 update app"]).unwrap();

    let snapshot = FileProjectStateStore::new(&durable_root)
        .rebuild_projection()
        .unwrap();
    assert!(snapshot.source_fingerprints.contains_key("git:HEAD"));
    let changes = snapshot.list_changes(ChangeProjectionQuery {
        q: Some("GAP1 update".to_string()),
        gap_id: Some("GAP1".to_string()),
        status: Some(GapStatus::Done),
        priority: Some("high".to_string()),
        page: PageRequest::default(),
        ..ChangeProjectionQuery::default()
    });
    assert_eq!(changes.total, 1);
    assert_eq!(changes.changes[0].gap_id.as_deref(), Some("GAP1"));
    assert_eq!(
        changes.changes[0].gap_name.as_deref(),
        Some("Change-linked Gap")
    );
    assert_eq!(changes.changes[0].gap_status, Some(GapStatus::Done));
    assert_eq!(changes.changes[0].gap_priority.as_deref(), Some("high"));

    fs::remove_dir_all(temp_root).unwrap();
}

fn gap_projection(id: &str, status: GapStatus, node_id: Option<&str>) -> GapSummaryProjection {
    GapSummaryProjection {
        gap: GapIndexProjection {
            id: id.to_string(),
            name: id.to_string(),
            status,
            priority: GapPriority::Medium,
            reporter: None,
            round_count: 0,
            created: "created".to_string(),
            updated: "updated".to_string(),
            branch_name: None,
            node_id: node_id.map(str::to_string),
            feature_id: None,
            feature_order: None,
            json_path: format!("{id}/gap.json"),
        },
        node_display_name: None,
        searchable_text: id.to_string(),
        activity_ids: Vec::new(),
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn git(root: &Path, args: &[&str]) -> RefineResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RefineError::Conflict(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }
}
