use std::path::PathBuf;

use super::*;

#[test]
fn file_metrics_service_records_summarizes_filters_and_prunes() {
    let temp_root = unique_temp_dir("metrics");
    let service = FileMetricsService::new(temp_root.join("run/8080"));
    service
        .record_event(PerformanceEvent {
            id: "old".to_string(),
            occurred_at: "2020-01-01T00:00:00Z".to_string(),
            operation: "cache.rebuild".to_string(),
            elapsed_ms: 50.0,
            success: true,
            goal_id: None,
            provider: None,
            query_mode: Some("full".to_string()),
            rows_returned: Some(3),
            rows_scanned: Some(5),
            details: json!({"resource_backend": "jsonl"}),
        })
        .unwrap();
    service
        .record_operation("cache.rebuild", 100.0, false, json!({}))
        .unwrap();
    service
        .record(MetricSample {
            name: "activity.query".to_string(),
            value: 10.0,
            tags: vec![("success".to_string(), "true".to_string())],
        })
        .unwrap();

    let report = service
        .report(PerformanceQuery {
            limit: 10,
            offset: 0,
            operation: Some("cache.rebuild".to_string()),
            success: None,
        })
        .unwrap();
    assert_eq!(report.filtered_event_count, 2);
    assert_eq!(report.summary.len(), 2);
    assert_eq!(report.operations, vec!["activity.query", "cache.rebuild"]);
    assert_eq!(service.query("activity.query").unwrap()[0].value, 10.0);

    let cleaned = service.cleanup(false).unwrap();
    assert_eq!(cleaned.deleted, 1);
    assert_eq!(
        service
            .report(PerformanceQuery::default())
            .unwrap()
            .total_event_count,
        2
    );

    let cleared = service.cleanup(true).unwrap();
    assert_eq!(cleared.deleted, 2);
    assert!(!service.path().exists());

    let _ = fs::remove_dir_all(temp_root);
}

#[test]
fn metrics_are_memory_resident_bounded_and_remove_the_legacy_log() {
    let temp_root = unique_temp_dir("metrics-memory");
    let runtime_root = temp_root.join("run/8080");
    let service = FileMetricsService::new(&runtime_root);

    // A legacy on-disk log from an older build is removed by cleanup so
    // upgraded installations reclaim the disk.
    fs::create_dir_all(service.path().parent().unwrap()).unwrap();
    fs::write(service.path(), "legacy\n").unwrap();
    service.cleanup(false).unwrap();
    assert!(!service.path().exists());

    // Recording never touches the filesystem.
    service
        .record_operation("http.request", 1.0, true, json!({"path": "/nodes"}))
        .unwrap();
    assert!(!service.path().exists());
    assert!(!service.path().parent().unwrap().exists());

    // The ring keeps the newest events once capacity is exceeded.
    for index in 0..(RECENT_EVENT_CAPACITY + 10) {
        service
            .record_operation("http.request", index as f64, true, json!({}))
            .unwrap();
    }
    let report = service.report(PerformanceQuery::default()).unwrap();
    assert_eq!(report.total_event_count, RECENT_EVENT_CAPACITY);

    service.cleanup(true).unwrap();
    let _ = fs::remove_dir_all(temp_root);
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
