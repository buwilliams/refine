use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Utc};

use crate::model::log::ActivityEntry;
use crate::process::supervisor::errors::{RefineError, RefineResult};

pub const ACTIVITY_LOG_FILE: &str = "logs/activity.jsonl";

pub trait ActivityService {
    fn append(&self, entry: ActivityEntry) -> RefineResult<()>;
    fn recent(&self, limit: usize) -> RefineResult<Vec<ActivityEntry>>;
    fn by_goal(&self, goal_id: &str, limit: usize) -> RefineResult<Vec<ActivityEntry>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityCleanupResult {
    pub ok: bool,
    pub deleted: usize,
    pub retained: usize,
    pub cleared: bool,
}

#[derive(Clone, Debug)]
pub struct FileActivityService {
    pub refine_dir: PathBuf,
}

impl FileActivityService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.refine_dir.join(ACTIVITY_LOG_FILE)
    }

    pub fn new_entry(
        &self,
        message: impl Into<String>,
        severity: impl Into<String>,
        category: impl Into<String>,
        goal_id: Option<String>,
        actor: Option<String>,
    ) -> ActivityEntry {
        ActivityEntry {
            id: new_activity_id(),
            datetime: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            severity: severity.into(),
            category: category.into(),
            message: message.into(),
            goal_id,
            actor,
            details: None,
            actions: Vec::new(),
        }
    }

    pub fn query(
        &self,
        limit: usize,
        offset: usize,
        goal_id: Option<&str>,
        severity: Option<&str>,
        category: Option<&str>,
        actor: Option<&str>,
        q: Option<&str>,
    ) -> RefineResult<Vec<ActivityEntry>> {
        let mut entries = self.read_all()?;
        entries.retain(|entry| {
            if let Some(goal_id) = goal_id {
                if entry.goal_id.as_deref() != Some(goal_id) {
                    return false;
                }
            }
            if let Some(severity) = severity {
                if entry.severity != severity {
                    return false;
                }
            }
            if let Some(category) = category {
                if entry.category != category {
                    return false;
                }
            }
            if let Some(actor) = actor {
                if entry.actor.as_deref() != Some(actor) {
                    return false;
                }
            }
            if let Some(query) = q {
                let query = query.to_lowercase();
                if !entry.message.to_lowercase().contains(&query)
                    && !entry.category.to_lowercase().contains(&query)
                    && !entry.severity.to_lowercase().contains(&query)
                    && !entry
                        .details
                        .as_ref()
                        .and_then(|details| serde_json::to_string(details).ok())
                        .map(|details| details.to_lowercase().contains(&query))
                        .unwrap_or(false)
                {
                    return false;
                }
            }
            true
        });
        entries.sort_by(|a, b| b.datetime.cmp(&a.datetime).then_with(|| b.id.cmp(&a.id)));
        Ok(entries.into_iter().skip(offset).take(limit).collect())
    }

    pub fn count(&self) -> RefineResult<usize> {
        Ok(self.read_all()?.len())
    }

    pub fn cleanup(&self, days: i64, clear: bool) -> RefineResult<ActivityCleanupResult> {
        let entries = self.read_all()?;
        let existing = entries.len();
        let path = self.path();
        if clear || days <= 0 {
            if path.exists() {
                fs::remove_file(&path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to remove activity log {}: {error}",
                        path.display()
                    ))
                })?;
            }
            return Ok(ActivityCleanupResult {
                ok: true,
                deleted: existing,
                retained: 0,
                cleared: true,
            });
        }

        let cutoff = Utc::now() - Duration::days(days);
        let mut retained = Vec::new();
        for entry in entries {
            let keep = chrono::DateTime::parse_from_rfc3339(&entry.datetime)
                .map(|parsed| parsed.with_timezone(&Utc) >= cutoff)
                .unwrap_or(true);
            if keep {
                retained.push(entry);
            }
        }
        let deleted = existing.saturating_sub(retained.len());
        if deleted > 0 {
            self.replace_all(&retained)?;
        }
        Ok(ActivityCleanupResult {
            ok: true,
            deleted,
            retained: retained.len(),
            cleared: false,
        })
    }

    pub fn facets(&self) -> RefineResult<serde_json::Value> {
        let entries = self.read_all()?;
        let mut categories = entries
            .iter()
            .map(|entry| entry.category.clone())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let mut severities = entries
            .iter()
            .map(|entry| entry.severity.clone())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let mut actors = entries
            .iter()
            .filter_map(|entry| entry.actor.clone())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        categories.sort();
        categories.dedup();
        severities.sort();
        severities.dedup();
        actors.sort();
        actors.dedup();
        Ok(serde_json::json!({
            "categories": categories,
            "severities": severities,
            "actors": actors
        }))
    }

    fn replace_all(&self, entries: &[ActivityEntry]) -> RefineResult<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create activity directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let temp_path = path.with_extension("jsonl.tmp");
        {
            let mut file = fs::File::create(&temp_path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create activity temp log {}: {error}",
                    temp_path.display()
                ))
            })?;
            for entry in entries {
                let encoded = serde_json::to_string(entry).map_err(|error| {
                    RefineError::Serialization(format!("failed to encode activity entry: {error}"))
                })?;
                writeln!(file, "{encoded}").map_err(|error| {
                    RefineError::Io(format!(
                        "failed to write activity temp log {}: {error}",
                        temp_path.display()
                    ))
                })?;
            }
        }
        fs::rename(&temp_path, &path).map_err(|error| {
            RefineError::Io(format!(
                "failed to replace activity log {} with {}: {error}",
                path.display(),
                temp_path.display()
            ))
        })
    }

    fn read_all(&self) -> RefineResult<Vec<ActivityEntry>> {
        let path = self.path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open activity log {}: {error}",
                path.display()
            ))
        })?;
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read activity log {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<ActivityEntry>(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse activity entry in {}: {error}",
                    path.display()
                ))
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }
}

impl ActivityService for FileActivityService {
    fn append(&self, entry: ActivityEntry) -> RefineResult<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create activity directory {}: {error}",
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
                    "failed to open activity log {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(&entry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode activity entry: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append activity log {}: {error}",
                path.display()
            ))
        })
    }

    fn recent(&self, limit: usize) -> RefineResult<Vec<ActivityEntry>> {
        self.query(limit, 0, None, None, None, None, None)
    }

    fn by_goal(&self, goal_id: &str, limit: usize) -> RefineResult<Vec<ActivityEntry>> {
        self.query(limit, 0, Some(goal_id), None, None, None, None)
    }
}

fn new_activity_id() -> String {
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
    use super::*;

    #[test]
    fn file_activity_service_appends_and_queries_jsonl() {
        let temp_root = unique_temp_dir("activity");
        let refine_dir = temp_root.join(".refine");
        let service = FileActivityService::new(&refine_dir);
        let entry = service.new_entry(
            "Something happened",
            "error",
            "ui",
            Some("GOAL1".to_string()),
            Some("browser".to_string()),
        );
        service.append(entry).unwrap();

        assert!(service.path().exists());
        assert_eq!(service.recent(10).unwrap().len(), 1);
        assert_eq!(service.by_goal("GOAL1", 10).unwrap()[0].category, "ui");
        assert_eq!(
            service
                .query(10, 0, None, Some("error"), None, None, None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service.facets().unwrap()["categories"],
            serde_json::json!(["ui"])
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_activity_service_prunes_old_entries() {
        let temp_root = unique_temp_dir("activity-cleanup");
        let refine_dir = temp_root.join(".refine");
        let service = FileActivityService::new(&refine_dir);
        let mut old = service.new_entry("Old", "info", "system", None, None);
        old.datetime = "2020-01-01T00:00:00Z".to_string();
        let recent = service.new_entry("Recent", "info", "system", None, None);
        service.append(old).unwrap();
        service.append(recent).unwrap();

        let cleaned = service.cleanup(30, false).unwrap();
        assert_eq!(cleaned.deleted, 1);
        assert_eq!(cleaned.retained, 1);
        assert_eq!(service.recent(10).unwrap()[0].message, "Recent");

        let cleared = service.cleanup(0, false).unwrap();
        assert_eq!(cleared.deleted, 1);
        assert_eq!(cleared.retained, 0);
        assert!(!service.path().exists());

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
