use crate::model::feature::FeatureRollup;
use crate::model::workflow::GoalStatus;
use crate::tools::observability::activity::ACTIVITY_LOG_FILE;
use crate::tools::observability::logs::FileLogService;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::model::feature::FeatureIndexProjection;
use crate::model::goal::{GoalIndexProjection, GoalPriority};
use crate::model::log::{ActivityEntry, LogEntry};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn projection_query_filters_sorts_and_pages_goals_and_features() {
    let mut goal_one = goal_projection("goal-1", GoalStatus::Todo, Some("default"));
    goal_one.goal.name = "OAuth callback broken".to_string();
    goal_one.goal.reporter = Some("Alice".to_string());
    goal_one.goal.round_count = 2;
    goal_one.goal.feature_id = Some("feature-1".to_string());
    goal_one.goal.priority = GoalPriority::High;
    goal_one.searchable_text = "OAuth callback broken login notes".to_string();
    goal_one.activity_ids = vec!["act-1".to_string()];

    let mut goal_two = goal_projection("goal-2", GoalStatus::Done, Some("node-b"));
    goal_two.goal.name = "Settings polish".to_string();
    goal_two.goal.reporter = Some("Bob".to_string());
    goal_two.goal.round_count = 1;

    let mut goals = BTreeMap::new();
    goals.insert(goal_one.goal.id.clone(), goal_one);
    goals.insert(goal_two.goal.id.clone(), goal_two);

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
                goal_id: Some("goal-1".to_string()),
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
            assignee: Some("Alice".to_string()),
            node_id: Some("default".to_string()),
            created: "created".to_string(),
            updated: "updated".to_string(),
            json_path: "feature.json".to_string(),
        },
        status: GoalStatus::Todo,
        goal_ids: vec!["goal-1".to_string()],
        rollup: FeatureRollup {
            status: GoalStatus::Todo,
            goal_count: 1,
            done_count: 0,
            active_count: 0,
            failed_count: 0,
            cancelled_count: 0,
            blocked_count: 0,
            next_goal: Some("goal-1".to_string()),
        },
    };
    let mut features = BTreeMap::new();
    features.insert("feature-1".to_string(), feature);

    let snapshot = ProjectionSnapshot {
        version: PROJECTION_SNAPSHOT_VERSION,
        generated_at: "now".to_string(),
        source_fingerprints: BTreeMap::new(),
        goals,
        features,
        activity,
        changes: BTreeMap::new(),
        dashboard: DashboardProjection::default(),
        runtime: RuntimeProjection::default(),
    };

    let goals = snapshot.list_goals(GoalProjectionQuery {
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
        ..GoalProjectionQuery::default()
    });
    assert_eq!(goals.total, 1);
    assert_eq!(goals.goals[0].id, "goal-1");
    assert_eq!(
        goals.filtered_status_counts.get(&GoalStatus::Todo),
        Some(&1)
    );
    assert_eq!(goals.matching_ids, vec!["goal-1"]);

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
        assignee: None,
        status: Some(GoalStatus::Todo),
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
fn rebuild_projection_scans_python_style_goal_and_feature_records() {
    let temp_root = unique_temp_dir("projection-rebuild");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    let remote_goal_dir = refine_dir.join("goals").join("02").join("GOAL2");
    let feature_dir = refine_dir.join("features").join("01").join("FEATURE1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::create_dir_all(&remote_goal_dir).unwrap();
    fs::create_dir_all(&feature_dir).unwrap();
    fs::create_dir_all(refine_dir.join("logs")).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Fix login",
              "status": "todo",
              "priority": "high",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "reporter": "Buddy",
              "feature_id": "FEATURE1",
              "feature_order": 2,
              "rounds": [
                {"reporter": "Buddy", "assignee": "Alice", "prompt": "Works"},
                {"reporter": "Reviewer", "assignee": "Coder", "prompt": "Works"}
              ],
              "notes": [{"body": "OAuth path"}]
            }"#,
    )
    .unwrap();
    fs::write(
        remote_goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL2",
              "name": "Remote failure",
              "status": "failed",
              "priority": "medium",
              "node_id": "node-b",
              "rounds": [{"reporter": "Remote", "prompt": "Fixed"}],
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
            refine_dir.join(ACTIVITY_LOG_FILE),
            concat!(
                "{\"id\":\"act-1\",\"datetime\":\"2026-01-03T00:00:00Z\",\"severity\":\"error\",\"category\":\"quality\",\"message\":\"Remote QA failed\",\"goal_id\":\"GOAL2\",\"actor\":\"browser\",\"details\":{\"selector\":\"#app\"},\"actions\":[]}\n",
                "{\"id\":\"act-2\",\"datetime\":\"2026-01-04T00:00:00Z\",\"severity\":\"info\",\"category\":\"state\",\"message\":\"Feature changed\",\"goal_id\":null,\"actor\":\"system\",\"details\":null,\"actions\":[]}\n"
            ),
        )
        .unwrap();
    FileLogService::new(&refine_dir)
        .append_round_log(
            "GOAL1",
            1,
            LogEntry {
                datetime: "2026-01-05T00:00:00Z".to_string(),
                severity: "warn".to_string(),
                category: "workflow".to_string(),
                message: "Round sidecar activity".to_string(),
                details: None,
                actions: Vec::new(),
                actor: Some("workflow".to_string()),
                goal_id: None,
            },
        )
        .unwrap();

    let snapshot = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let goal = &snapshot.goals["GOAL1"];
    assert_eq!(goal.goal.status, GoalStatus::Todo);
    assert_eq!(goal.goal.priority, GoalPriority::High);
    assert_eq!(goal.goal.reporter.as_deref(), Some("Buddy"));
    assert_eq!(goal.goal.assignee.as_deref(), Some("Coder"));
    assert_eq!(goal.goal.round_count, 2);
    assert_eq!(goal.goal.node_id.as_deref(), Some("default"));
    assert!(goal.searchable_text.contains("OAuth path"));
    assert!(goal.searchable_text.contains("Coder"));

    let feature = &snapshot.features["FEATURE1"];
    assert_eq!(feature.goal_ids, vec!["GOAL1"]);
    assert_eq!(feature.rollup.goal_count, 1);
    assert_eq!(feature.rollup.next_goal.as_deref(), Some("GOAL1"));
    assert!(
        snapshot
            .source_fingerprints
            .contains_key("goals/01/GOAL1/goal.json")
    );
    assert!(
        snapshot.source_fingerprints["goals/01/GOAL1/goal.json"]
            .content_hash
            .is_some()
    );
    assert_eq!(
        snapshot
            .dashboard
            .all_node_status_counts
            .get(&GoalStatus::Todo),
        Some(&1)
    );
    assert_eq!(
        snapshot
            .dashboard
            .all_node_status_counts
            .get(&GoalStatus::Failed),
        Some(&1)
    );
    assert_eq!(
        snapshot
            .dashboard
            .current_node_status_counts
            .get(&GoalStatus::Todo),
        Some(&1)
    );
    assert_eq!(
        snapshot
            .dashboard
            .current_node_status_counts
            .get(&GoalStatus::Failed),
        None
    );
    assert_eq!(snapshot.dashboard.attention_indicators.len(), 1);
    assert_eq!(
        snapshot
            .dashboard
            .assignee_stats
            .get("Coder")
            .and_then(|counts| counts.get(&GoalStatus::Todo)),
        Some(&1)
    );
    let default_dashboard = snapshot.dashboard_summary(DashboardProjectionQuery {
        node: Some("current".to_string()),
        current_node_id: Some("default".to_string()),
    });
    assert_eq!(default_dashboard.node_filter, "current");
    assert_eq!(default_dashboard.counts.get(&GoalStatus::Todo), Some(&1));
    assert_eq!(default_dashboard.counts.get(&GoalStatus::Failed), None);
    assert_eq!(
        default_dashboard
            .assignee_stats
            .get("Coder")
            .and_then(|counts| counts.get(&GoalStatus::Todo)),
        Some(&1)
    );
    assert!(!default_dashboard.assignee_stats.contains_key("unassigned"));
    assert_eq!(
        default_dashboard.recent_activity_ids,
        vec!["round-log:GOAL1:1:0".to_string()]
    );
    let remote_dashboard = snapshot.dashboard_summary(DashboardProjectionQuery {
        node: Some("current".to_string()),
        current_node_id: Some("node-b".to_string()),
    });
    assert_eq!(remote_dashboard.counts.get(&GoalStatus::Failed), Some(&1));
    assert_eq!(remote_dashboard.counts.get(&GoalStatus::Todo), None);
    assert_eq!(remote_dashboard.attention_indicators.len(), 1);
    assert_eq!(
        remote_dashboard.recent_activity_ids,
        vec!["act-1".to_string()]
    );
    let all_dashboard = snapshot.dashboard_summary(DashboardProjectionQuery {
        node: Some("all".to_string()),
        current_node_id: Some("node-b".to_string()),
    });
    assert_eq!(all_dashboard.counts, all_dashboard.all_node_counts);
    assert_eq!(
        all_dashboard.recent_activity_ids,
        vec![
            "round-log:GOAL1:1:0".to_string(),
            "act-2".to_string(),
            "act-1".to_string()
        ]
    );
    assert_eq!(snapshot.activity.len(), 3);
    assert_eq!(
        snapshot.goals["GOAL1"].activity_ids,
        vec!["round-log:GOAL1:1:0"]
    );
    assert_eq!(snapshot.goals["GOAL2"].activity_ids, vec!["act-1"]);
    assert!(snapshot.activity["act-1"].searchable_text.contains("#app"));
    assert_eq!(
        snapshot.activity["round-log:GOAL1:1:0"].entry.message,
        "Round sidecar activity"
    );
    assert_eq!(
        snapshot.activity["round-log:GOAL1:1:0"]
            .entry
            .goal_id
            .as_deref(),
        Some("GOAL1")
    );
    assert_eq!(
        snapshot.dashboard.recent_activity_ids,
        vec![
            "round-log:GOAL1:1:0".to_string(),
            "act-2".to_string(),
            "act-1".to_string()
        ]
    );
    assert!(
        snapshot
            .source_fingerprints
            .contains_key("logs/activity.jsonl")
    );
    assert!(
        snapshot
            .source_fingerprints
            .contains_key("goals/GO/AL1/logs.jsonl")
    );
    let goal_activity = snapshot.list_activity(ActivityProjectionQuery {
        goal_id: Some("GOAL1".to_string()),
        ..ActivityProjectionQuery::default()
    });
    assert_eq!(goal_activity.total, 1);
    assert_eq!(goal_activity.activity[0].message, "Round sidecar activity");
    let activity_filtered = snapshot.list_goals(GoalProjectionQuery {
        severity: Some("error".to_string()),
        category: Some("quality".to_string()),
        actor: Some("browser".to_string()),
        ..GoalProjectionQuery::default()
    });
    assert_eq!(activity_filtered.matching_ids, vec!["GOAL2"]);

    fs::remove_dir_all(temp_root).unwrap();
}

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

fn goal_projection(id: &str, status: GoalStatus, node_id: Option<&str>) -> GoalSummaryProjection {
    GoalSummaryProjection {
        goal: GoalIndexProjection {
            id: id.to_string(),
            name: id.to_string(),
            status,
            priority: GoalPriority::Medium,
            reporter: None,
            assignee: None,
            round_count: 0,
            created: "created".to_string(),
            updated: "updated".to_string(),
            branch_name: None,
            node_id: node_id.map(str::to_string),
            feature_id: None,
            feature_order: None,
            json_path: format!("{id}/goal.json"),
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
