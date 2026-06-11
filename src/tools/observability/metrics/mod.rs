use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::process::supervisor::errors::{RefineError, RefineResult};

pub const METRICS_LOG_FILE: &str = "metrics/performance.jsonl";
pub const DEFAULT_METRICS_RETENTION_DAYS: i64 = 30;

static METRICS_LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MetricSample {
    pub name: String,
    pub value: f64,
    #[serde(default)]
    pub tags: Vec<(String, String)>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PerformanceEvent {
    pub id: String,
    pub occurred_at: String,
    pub operation: String,
    pub elapsed_ms: f64,
    pub success: bool,
    pub gap_id: Option<String>,
    pub provider: Option<String>,
    pub query_mode: Option<String>,
    pub rows_returned: Option<u64>,
    pub rows_scanned: Option<u64>,
    pub details: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PerformanceQuery {
    pub limit: usize,
    pub offset: usize,
    pub operation: Option<String>,
    pub success: Option<bool>,
}

impl Default for PerformanceQuery {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            operation: None,
            success: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PerformanceSummaryRow {
    pub operation: String,
    pub count: usize,
    pub failures: usize,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub last_seen: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PerformancePage {
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PerformanceReport {
    pub summary: Vec<PerformanceSummaryRow>,
    pub recent: Vec<PerformanceEvent>,
    pub events: Vec<PerformanceEvent>,
    pub operations: Vec<String>,
    pub event_count: usize,
    pub filtered_event_count: usize,
    pub total_event_count: usize,
    pub retention_days: i64,
    pub page: PerformancePage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricsCleanupResult {
    pub ok: bool,
    pub deleted: usize,
    pub retained: usize,
    pub cleared: bool,
}

pub trait MetricsService {
    fn record(&self, sample: MetricSample) -> RefineResult<()>;
    fn query(&self, name: &str) -> RefineResult<Vec<MetricSample>>;
}

#[derive(Clone, Debug)]
pub struct FileMetricsService {
    pub runtime_root: PathBuf,
    pub retention_days: i64,
}

impl FileMetricsService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            retention_days: DEFAULT_METRICS_RETENTION_DAYS,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.runtime_root.join(METRICS_LOG_FILE)
    }

    pub fn record_event(&self, mut event: PerformanceEvent) -> RefineResult<PerformanceEvent> {
        if event.id.trim().is_empty() {
            event.id = new_metric_id();
        }
        if event.occurred_at.trim().is_empty() {
            event.occurred_at = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        }
        self.append_event(&event)?;
        Ok(event)
    }

    pub fn record_operation(
        &self,
        operation: impl Into<String>,
        elapsed_ms: f64,
        success: bool,
        details: Value,
    ) -> RefineResult<PerformanceEvent> {
        self.record_event(PerformanceEvent {
            id: String::new(),
            occurred_at: String::new(),
            operation: operation.into(),
            elapsed_ms,
            success,
            gap_id: None,
            provider: None,
            query_mode: None,
            rows_returned: None,
            rows_scanned: None,
            details,
        })
    }

    pub fn report(&self, query: PerformanceQuery) -> RefineResult<PerformanceReport> {
        let mut events = self.read_all()?;
        events.sort_by(|a, b| {
            b.occurred_at
                .cmp(&a.occurred_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let total_event_count = events.len();
        let operations = operations(&events);
        let summary = summary(&events);

        let mut filtered = events;
        if let Some(operation) = query.operation.as_deref().filter(|value| !value.is_empty()) {
            filtered.retain(|event| event.operation == operation);
        }
        if let Some(success) = query.success {
            filtered.retain(|event| event.success == success);
        }
        let filtered_event_count = filtered.len();
        let recent = filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect::<Vec<_>>();
        Ok(PerformanceReport {
            summary,
            event_count: recent.len(),
            events: recent.clone(),
            recent,
            operations,
            filtered_event_count,
            total_event_count,
            retention_days: self.retention_days,
            page: PerformancePage {
                limit: query.limit,
                offset: query.offset,
                has_more: query.offset + query.limit < filtered_event_count,
                total: filtered_event_count,
            },
        })
    }

    pub fn cleanup(&self, clear: bool) -> RefineResult<MetricsCleanupResult> {
        let events = self.read_all()?;
        let existing = events.len();
        let path = self.path();
        if clear {
            if path.exists() {
                fs::remove_file(&path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to remove metrics log {}: {error}",
                        path.display()
                    ))
                })?;
            }
            return Ok(MetricsCleanupResult {
                ok: true,
                deleted: existing,
                retained: 0,
                cleared: true,
            });
        }

        let cutoff = Utc::now() - Duration::days(self.retention_days);
        let mut retained = Vec::new();
        for event in events {
            let keep = chrono::DateTime::parse_from_rfc3339(&event.occurred_at)
                .map(|parsed| parsed.with_timezone(&Utc) >= cutoff)
                .unwrap_or(true);
            if keep {
                retained.push(event);
            }
        }
        let deleted = existing.saturating_sub(retained.len());
        if deleted > 0 {
            self.replace_all(&retained)?;
        }
        Ok(MetricsCleanupResult {
            ok: true,
            deleted,
            retained: retained.len(),
            cleared: false,
        })
    }

    fn append_event(&self, event: &PerformanceEvent) -> RefineResult<()> {
        let _guard = METRICS_LOG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| RefineError::Io("metrics log lock was poisoned".to_string()))?;
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create metrics directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open metrics log {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(event).map_err(|error| {
            RefineError::Serialization(format!("failed to encode metrics event: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append metrics log {}: {error}",
                path.display()
            ))
        })
    }

    fn read_all(&self) -> RefineResult<Vec<PerformanceEvent>> {
        let path = self.path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open metrics log {}: {error}",
                path.display()
            ))
        })?;
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read metrics log {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            events.extend(parse_performance_events_line(&line));
        }
        Ok(events)
    }

    fn replace_all(&self, events: &[PerformanceEvent]) -> RefineResult<()> {
        let _guard = METRICS_LOG_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| RefineError::Io("metrics log lock was poisoned".to_string()))?;
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create metrics directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let temp_path = path.with_extension("jsonl.tmp");
        {
            let mut file = fs::File::create(&temp_path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create metrics temp log {}: {error}",
                    temp_path.display()
                ))
            })?;
            for event in events {
                let encoded = serde_json::to_string(event).map_err(|error| {
                    RefineError::Serialization(format!("failed to encode metrics event: {error}"))
                })?;
                writeln!(file, "{encoded}").map_err(|error| {
                    RefineError::Io(format!(
                        "failed to write metrics temp log {}: {error}",
                        temp_path.display()
                    ))
                })?;
            }
        }
        fs::rename(&temp_path, &path).map_err(|error| {
            RefineError::Io(format!(
                "failed to replace metrics log {} with {}: {error}",
                path.display(),
                temp_path.display()
            ))
        })
    }
}

impl MetricsService for FileMetricsService {
    fn record(&self, sample: MetricSample) -> RefineResult<()> {
        let tags = sample.tags.into_iter().collect::<BTreeMap<_, _>>();
        let operation = tags
            .get("operation")
            .cloned()
            .unwrap_or_else(|| sample.name.clone());
        let success = tags
            .get("success")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let mut event = PerformanceEvent {
            id: String::new(),
            occurred_at: String::new(),
            operation,
            elapsed_ms: sample.value,
            success,
            gap_id: tags.get("gap_id").cloned(),
            provider: tags.get("provider").cloned(),
            query_mode: tags.get("query_mode").cloned(),
            rows_returned: tags
                .get("rows_returned")
                .and_then(|value| value.parse::<u64>().ok()),
            rows_scanned: tags
                .get("rows_scanned")
                .and_then(|value| value.parse::<u64>().ok()),
            details: json!({ "metric": sample.name, "tags": tags }),
        };
        if let Some(occurred_at) = event
            .details
            .get("tags")
            .and_then(|tags| tags.get("occurred_at"))
            .and_then(|value| value.as_str())
        {
            event.occurred_at = occurred_at.to_string();
        }
        self.record_event(event).map(|_| ())
    }

    fn query(&self, name: &str) -> RefineResult<Vec<MetricSample>> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|event| event.operation == name)
            .map(|event| MetricSample {
                name: event.operation.clone(),
                value: event.elapsed_ms,
                tags: event_tags(&event),
            })
            .collect())
    }
}

fn summary(events: &[PerformanceEvent]) -> Vec<PerformanceSummaryRow> {
    let mut grouped: BTreeMap<String, Vec<&PerformanceEvent>> = BTreeMap::new();
    for event in events {
        grouped
            .entry(event.operation.clone())
            .or_default()
            .push(event);
    }
    grouped
        .into_iter()
        .map(|(operation, rows)| {
            let mut elapsed = rows
                .iter()
                .map(|event| event.elapsed_ms)
                .collect::<Vec<_>>();
            elapsed.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let count = elapsed.len();
            let failures = rows.iter().filter(|event| !event.success).count();
            let total = elapsed.iter().copied().sum::<f64>();
            let max_ms = elapsed.last().copied().unwrap_or(0.0);
            let p95_index = if count == 0 {
                0
            } else {
                ((count as f64 * 0.95).ceil() as usize).saturating_sub(1)
            };
            let last_seen = rows
                .iter()
                .map(|event| event.occurred_at.clone())
                .max()
                .unwrap_or_default();
            PerformanceSummaryRow {
                operation,
                count,
                failures,
                avg_ms: if count == 0 {
                    0.0
                } else {
                    total / count as f64
                },
                p95_ms: elapsed.get(p95_index).copied().unwrap_or(max_ms),
                max_ms,
                last_seen,
            }
        })
        .collect()
}

fn operations(events: &[PerformanceEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| event.operation.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn parse_performance_events_line(line: &str) -> Vec<PerformanceEvent> {
    if let Ok(event) = serde_json::from_str::<PerformanceEvent>(line) {
        return vec![event];
    }
    let mut events = Vec::new();
    for event in serde_json::Deserializer::from_str(line).into_iter::<PerformanceEvent>() {
        match event {
            Ok(event) => events.push(event),
            Err(_) => break,
        }
    }
    events
}

fn event_tags(event: &PerformanceEvent) -> Vec<(String, String)> {
    let mut tags = vec![
        ("operation".to_string(), event.operation.clone()),
        (
            "success".to_string(),
            if event.success { "true" } else { "false" }.to_string(),
        ),
    ];
    if let Some(value) = &event.gap_id {
        tags.push(("gap_id".to_string(), value.clone()));
    }
    if let Some(value) = &event.provider {
        tags.push(("provider".to_string(), value.clone()));
    }
    if let Some(value) = &event.query_mode {
        tags.push(("query_mode".to_string(), value.clone()));
    }
    tags
}

fn new_metric_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{}-{}",
        now.as_millis(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
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
                gap_id: None,
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
            gap_id: None,
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
            gap_id: None,
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
}
