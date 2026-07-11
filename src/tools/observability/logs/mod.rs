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
    fn round_logs(&self, goal_id: &str, round_idx: usize) -> RefineResult<Vec<RoundLogEntry>>;
    fn tail(&self, category: Option<&str>) -> RefineResult<String>;
}

#[derive(Clone, Debug)]
pub struct FileLogService {
    pub refine_dir: PathBuf,
}

impl FileLogService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn append_round_log(
        &self,
        goal_id: &str,
        round_idx: usize,
        mut entry: LogEntry,
    ) -> RefineResult<RoundLogEntry> {
        if entry.datetime.trim().is_empty() {
            entry.datetime = now_timestamp();
        }
        if entry.goal_id.is_none() {
            entry.goal_id = Some(goal_id.to_string());
        }
        let round_entry = RoundLogEntry {
            entry,
            round_idx: Some(round_idx),
        };
        let path = goal_logs_path(&self.refine_dir, goal_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Goal log directory {}: {error}",
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
                    "failed to open Goal log sidecar {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(&round_entry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode round log entry: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append Goal log sidecar {}: {error}",
                path.display()
            ))
        })?;
        Ok(round_entry)
    }

    pub fn page_round_logs(
        &self,
        goal_id: &str,
        round_idx: usize,
        limit: usize,
        offset: usize,
    ) -> RefineResult<(Vec<RoundLogEntry>, bool, usize)> {
        let mut entries = self.round_logs(goal_id, round_idx)?;
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

    pub fn all_round_logs(&self, goal_id: &str) -> RefineResult<Vec<RoundLogEntry>> {
        self.read_sidecar(goal_id)
    }

    fn read_sidecar(&self, goal_id: &str) -> RefineResult<Vec<RoundLogEntry>> {
        let path = goal_logs_path(&self.refine_dir, goal_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open Goal log sidecar {}: {error}",
                path.display()
            ))
        })?;
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read Goal log sidecar {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = parse_round_log_line(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse Goal log sidecar {}: {error}",
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
            "goal_id": null,
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
        let goal_id = entry.goal_id.clone().ok_or_else(|| {
            RefineError::InvalidInput("round log goal_id is required".to_string())
        })?;
        self.append_round_log(&goal_id, 0, entry)?;
        Ok(())
    }

    fn query(&self, query: LogQuery) -> RefineResult<Vec<LogEntry>> {
        let goal_id = query
            .goal_id
            .ok_or_else(|| RefineError::InvalidInput("goal_id is required".to_string()))?;
        let round_idx = query.offset.unwrap_or(0);
        Ok(self
            .round_logs(&goal_id, round_idx)?
            .into_iter()
            .map(|entry| entry.entry)
            .collect())
    }

    fn round_logs(&self, goal_id: &str, round_idx: usize) -> RefineResult<Vec<RoundLogEntry>> {
        Ok(self
            .read_sidecar(goal_id)?
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

fn goal_logs_path(refine_dir: &Path, goal_id: &str) -> PathBuf {
    let goal_id = goal_id.to_uppercase();
    refine_dir
        .join("goals")
        .join(&goal_id[..2])
        .join(&goal_id[2..])
        .join("logs.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_log_service_appends_and_pages_round_sidecar() {
        let temp_root = unique_temp_dir("round-logs");
        let refine_dir = temp_root.join(".refine");
        let service = FileLogService::new(&refine_dir);
        let entry = LogEntry {
            datetime: String::new(),
            severity: "info".to_string(),
            category: "state".to_string(),
            message: "Workflow status changed: backlog -> todo".to_string(),
            details: None,
            actions: Vec::new(),
            actor: Some("refine".to_string()),
            goal_id: None,
        };
        let written = service.append_round_log("GOAL1", 1, entry).unwrap();
        assert_eq!(written.round_idx, Some(1));
        assert!(refine_dir.join("goals/GO/AL1/logs.jsonl").exists());

        let (page, has_more, total) = service.page_round_logs("GOAL1", 1, 50, 0).unwrap();
        assert!(!has_more);
        assert_eq!(total, 1);
        assert_eq!(page[0].entry.goal_id.as_deref(), Some("GOAL1"));
        assert_eq!(service.round_logs("GOAL1", 0).unwrap().len(), 0);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_log_service_tolerates_legacy_string_details_in_round_sidecar() {
        let temp_root = unique_temp_dir("round-logs-legacy-details");
        let refine_dir = temp_root.join(".refine");
        let path = goal_logs_path(&refine_dir, "GOAL1");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"datetime":"2026-06-06T00:00:00Z","severity":"warn","category":"quality","message":"Provider note","details":"Moving code into src/ may be acceptable","actions":[],"actor":"refine","goal_id":"GOAL1","round_idx":1}"#,
        )
        .unwrap();

        let service = FileLogService::new(&refine_dir);
        let logs = service.round_logs("GOAL1", 1).unwrap();

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
