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

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_metrics_service_tolerates_concatenated_jsonl_events() {
    let temp_root = unique_temp_dir("metrics-corrupt-jsonl");
    let service = FileMetricsService::new(temp_root.join("run/8080"));
    let event_a = PerformanceEvent {
        id: "a".to_string(),
        occurred_at: "2026-01-01T00:00:00Z".to_string(),
        operation: "http.request".to_string(),
        elapsed_ms: 1.0,
        success: true,
        goal_id: None,
        provider: None,
        query_mode: None,
        rows_returned: None,
        rows_scanned: None,
        details: json!({"path": "/nodes"}),
    };
    let event_b = PerformanceEvent {
        id: "b".to_string(),
        occurred_at: "2026-01-01T00:00:01Z".to_string(),
        operation: "http.request".to_string(),
        elapsed_ms: 2.0,
        success: true,
        goal_id: None,
        provider: None,
        query_mode: None,
        rows_returned: None,
        rows_scanned: None,
        details: json!({"path": "/dashboard"}),
    };
    fs::create_dir_all(service.path().parent().unwrap()).unwrap();
    fs::write(
        service.path(),
        format!(
            "{}{}\nnot-json\n",
            serde_json::to_string(&event_a).unwrap(),
            serde_json::to_string(&event_b).unwrap()
        ),
    )
    .unwrap();

    let report = service.report(PerformanceQuery::default()).unwrap();
    assert_eq!(report.total_event_count, 2);
    assert_eq!(report.events[0].id, "b");
    assert_eq!(report.events[1].id, "a");

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
