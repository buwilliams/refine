use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::log::{LogEntry, LogQuery, RoundLogEntry};
use crate::process::supervisor::errors::{RefineError, RefineResult};

pub trait LogService {
    fn append(&self, entry: LogEntry) -> RefineResult<()>;
    fn query(&self, query: LogQuery) -> RefineResult<Vec<LogEntry>>;
    fn round_logs(&self, gap_id: &str, round_idx: usize) -> RefineResult<Vec<RoundLogEntry>>;
    fn tail(&self, category: Option<&str>) -> RefineResult<String>;
}

#[derive(Clone, Debug)]
pub struct FileLogService {
    pub durable_root: PathBuf,
}

impl FileLogService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
        }
    }

    pub fn append_round_log(
        &self,
        gap_id: &str,
        round_idx: usize,
        mut entry: LogEntry,
    ) -> RefineResult<RoundLogEntry> {
        if entry.datetime.trim().is_empty() {
            entry.datetime = now_timestamp();
        }
        if entry.gap_id.is_none() {
            entry.gap_id = Some(gap_id.to_string());
        }
        let round_entry = RoundLogEntry {
            entry,
            round_idx: Some(round_idx),
        };
        let path = gap_logs_path(&self.durable_root, gap_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Gap log directory {}: {error}",
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
                    "failed to open Gap log sidecar {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(&round_entry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode round log entry: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append Gap log sidecar {}: {error}",
                path.display()
            ))
        })?;
        Ok(round_entry)
    }

    pub fn page_round_logs(
        &self,
        gap_id: &str,
        round_idx: usize,
        limit: usize,
        offset: usize,
    ) -> RefineResult<(Vec<RoundLogEntry>, bool, usize)> {
        let mut entries = self.round_logs(gap_id, round_idx)?;
        entries.sort_by(|a, b| {
            a.entry
                .datetime
                .cmp(&b.entry.datetime)
                .then_with(|| a.entry.message.cmp(&b.entry.message))
        });
        let total = entries.len();
        let limit = limit.clamp(1, 200);
        let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
        let has_more = offset + page.len() < total;
        Ok((page, has_more, total))
    }

    fn read_sidecar(&self, gap_id: &str) -> RefineResult<Vec<RoundLogEntry>> {
        let path = gap_logs_path(&self.durable_root, gap_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open Gap log sidecar {}: {error}",
                path.display()
            ))
        })?;
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read Gap log sidecar {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = parse_round_log_line(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse Gap log sidecar {}: {error}",
                    path.display()
                ))
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }
}

fn parse_round_log_line(line: &str) -> serde_json::Result<RoundLogEntry> {
    let value = serde_json::from_str::<Value>(line)?;
    serde_json::from_value(normalize_round_log_value(value))
}

fn normalize_round_log_value(value: Value) -> Value {
    match value {
        Value::String(message) => json!({
            "datetime": now_timestamp(),
            "severity": "info",
            "category": "state",
            "message": message,
            "details": null,
            "actions": [],
            "actor": null,
            "gap_id": null,
            "round_idx": null
        }),
        Value::Object(mut object) => {
            if object
                .get("details")
                .is_some_and(|details| !details.is_object() && !details.is_null())
                && let Some(details) = object.remove("details")
            {
                object.insert("details".to_string(), json!({ "value": details }));
            }
            if object
                .get("actions")
                .is_some_and(|actions| !actions.is_array() && !actions.is_null())
            {
                object.insert("actions".to_string(), json!([]));
            }
            Value::Object(object)
        }
        other => other,
    }
}

impl LogService for FileLogService {
    fn append(&self, entry: LogEntry) -> RefineResult<()> {
        let gap_id = entry
            .gap_id
            .clone()
            .ok_or_else(|| RefineError::InvalidInput("round log gap_id is required".to_string()))?;
        self.append_round_log(&gap_id, 0, entry)?;
        Ok(())
    }

    fn query(&self, query: LogQuery) -> RefineResult<Vec<LogEntry>> {
        let gap_id = query
            .gap_id
            .ok_or_else(|| RefineError::InvalidInput("gap_id is required".to_string()))?;
        let round_idx = query.offset.unwrap_or(0);
        Ok(self
            .round_logs(&gap_id, round_idx)?
            .into_iter()
            .map(|entry| entry.entry)
            .collect())
    }

    fn round_logs(&self, gap_id: &str, round_idx: usize) -> RefineResult<Vec<RoundLogEntry>> {
        Ok(self
            .read_sidecar(gap_id)?
            .into_iter()
            .filter(|entry| entry.round_idx == Some(round_idx))
            .collect())
    }

    fn tail(&self, category: Option<&str>) -> RefineResult<String> {
        Ok(format!(
            "round-log tail category={}",
            category.unwrap_or("all")
        ))
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn gap_logs_path(durable_root: &Path, gap_id: &str) -> PathBuf {
    let gap_id = gap_id.to_uppercase();
    durable_root
        .join("gaps")
        .join(&gap_id[..2])
        .join(&gap_id[2..])
        .join("logs.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_log_service_appends_and_pages_round_sidecar() {
        let temp_root = unique_temp_dir("round-logs");
        let durable_root = temp_root.join(".refine");
        let service = FileLogService::new(&durable_root);
        let entry = LogEntry {
            datetime: String::new(),
            severity: "info".to_string(),
            category: "state".to_string(),
            message: "Workflow status changed: backlog -> todo".to_string(),
            details: None,
            actions: Vec::new(),
            actor: Some("refine".to_string()),
            gap_id: None,
        };
        let written = service.append_round_log("GAP1", 1, entry).unwrap();
        assert_eq!(written.round_idx, Some(1));
        assert!(durable_root.join("gaps/GA/P1/logs.jsonl").exists());

        let (page, has_more, total) = service.page_round_logs("GAP1", 1, 50, 0).unwrap();
        assert!(!has_more);
        assert_eq!(total, 1);
        assert_eq!(page[0].entry.gap_id.as_deref(), Some("GAP1"));
        assert_eq!(service.round_logs("GAP1", 0).unwrap().len(), 0);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_log_service_tolerates_legacy_string_details_in_round_sidecar() {
        let temp_root = unique_temp_dir("round-logs-legacy-details");
        let durable_root = temp_root.join(".refine");
        let path = gap_logs_path(&durable_root, "GAP1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"datetime":"2026-06-06T00:00:00Z","severity":"warn","category":"quality","message":"Provider note","details":"Moving code into src/ may be acceptable","actions":[],"actor":"refine","gap_id":"GAP1","round_idx":1}"#,
        )
        .unwrap();

        let service = FileLogService::new(&durable_root);
        let logs = service.round_logs("GAP1", 1).unwrap();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].entry.message, "Provider note");
        assert_eq!(
            logs[0].entry.details.as_ref().unwrap()["value"],
            "Moving code into src/ may be acceptable"
        );

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
