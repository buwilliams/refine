use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::tools::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum JobState {
    Pending,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl JobState {
    pub fn as_api_status(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobHandle {
    pub id: String,
    pub owner: String,
    pub state: JobState,
    #[serde(default = "empty_object")]
    pub progress: Value,
    #[serde(default = "empty_object")]
    pub result: Value,
    #[serde(default)]
    pub error: Option<Value>,
}

pub trait JobRegistry {
    fn register(&self, owner: &str) -> RefineResult<JobHandle>;
    fn status(&self, job_id: &str) -> RefineResult<JobHandle>;
    fn cancel(&self, job_id: &str) -> RefineResult<JobHandle>;
    fn recover(&self) -> RefineResult<Vec<JobHandle>>;
}

#[derive(Clone, Debug)]
pub struct FileJobRegistry {
    pub runtime_root: PathBuf,
}

impl FileJobRegistry {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.runtime_root.join("jobs")
    }

    fn job_path(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(format!("{job_id}.json"))
    }

    fn log_path(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(format!("{job_id}.logs.jsonl"))
    }

    fn write(&self, handle: &JobHandle) -> RefineResult<()> {
        fs::create_dir_all(self.jobs_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create job registry {}: {error}",
                self.jobs_dir().display()
            ))
        })?;
        let path = self.job_path(&handle.id);
        let encoded = serde_json::to_vec_pretty(handle).map_err(|error| {
            RefineError::Serialization(format!("failed to encode job: {error}"))
        })?;
        fs::write(&path, encoded).map_err(|error| {
            RefineError::Io(format!("failed to write job {}: {error}", path.display()))
        })
    }

    pub fn interrupt_active(&self) -> RefineResult<Vec<JobHandle>> {
        let mut interrupted = Vec::new();
        for mut job in self.recover()? {
            if matches!(
                job.state,
                JobState::Pending | JobState::Running | JobState::Cancelling
            ) {
                job.state = JobState::Interrupted;
                self.write(&job)?;
                self.append_log(
                    &job.id,
                    job_log_entry(&job, "warning", "Job interrupted", None),
                )?;
                interrupted.push(job);
            }
        }
        Ok(interrupted)
    }

    pub fn append_log(&self, job_id: &str, mut entry: LogEntry) -> RefineResult<LogEntry> {
        let job = self.status(job_id)?;
        if entry.datetime.trim().is_empty() {
            entry.datetime = now_timestamp();
        }
        if entry.category.trim().is_empty() {
            entry.category = "job".to_string();
        }
        if entry.actor.is_none() {
            entry.actor = Some("refine".to_string());
        }
        let mut details = entry.details.unwrap_or_default();
        details
            .entry("job_id".to_string())
            .or_insert_with(|| json!(job.id));
        details
            .entry("owner".to_string())
            .or_insert_with(|| json!(job.owner));
        details
            .entry("state".to_string())
            .or_insert_with(|| json!(job.state));
        entry.details = Some(details);
        fs::create_dir_all(self.jobs_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create job log registry {}: {error}",
                self.jobs_dir().display()
            ))
        })?;
        let path = self.log_path(job_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open job log {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(&entry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode job log: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append job log {}: {error}",
                path.display()
            ))
        })?;
        Ok(entry)
    }

    pub fn page_logs(
        &self,
        job_id: &str,
        limit: usize,
        offset: usize,
    ) -> RefineResult<(Vec<LogEntry>, bool, usize)> {
        self.status(job_id)?;
        let path = self.log_path(job_id);
        if !path.exists() {
            return Ok((Vec::new(), false, 0));
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open job log {}: {error}",
                path.display()
            ))
        })?;
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read job log {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<LogEntry>(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse job log {}: {error}",
                    path.display()
                ))
            })?;
            entries.push(entry);
        }
        entries.sort_by(|a, b| {
            a.datetime
                .cmp(&b.datetime)
                .then_with(|| a.message.cmp(&b.message))
        });
        let total = entries.len();
        let limit = limit.clamp(1, 200);
        let page = entries
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let has_more = offset + page.len() < total;
        Ok((page, has_more, total))
    }

    pub fn finish(&self, job_id: &str, state: JobState) -> RefineResult<JobHandle> {
        if !matches!(
            state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled | JobState::Interrupted
        ) {
            return Err(RefineError::InvalidInput(
                "finished jobs must use a terminal state".to_string(),
            ));
        }
        let mut handle = self.status(job_id)?;
        handle.state = state;
        self.write(&handle)?;
        self.append_log(
            &handle.id,
            job_log_entry(&handle, "info", "Job finished", None),
        )?;
        Ok(handle)
    }

    pub fn update_progress(&self, job_id: &str, progress: Value) -> RefineResult<JobHandle> {
        let mut handle = self.status(job_id)?;
        handle.progress = progress;
        self.write(&handle)?;
        Ok(handle)
    }

    pub fn finish_with_result(
        &self,
        job_id: &str,
        state: JobState,
        result: Value,
    ) -> RefineResult<JobHandle> {
        if !matches!(state, JobState::Succeeded | JobState::Failed) {
            return Err(RefineError::InvalidInput(
                "result jobs must finish as succeeded or failed".to_string(),
            ));
        }
        let mut handle = self.status(job_id)?;
        handle.state = state;
        handle.result = result;
        handle.error = None;
        self.write(&handle)?;
        self.append_log(
            &handle.id,
            job_log_entry(&handle, "info", "Job finished", None),
        )?;
        Ok(handle)
    }

    pub fn fail_with_error(&self, job_id: &str, error: Value) -> RefineResult<JobHandle> {
        let mut handle = self.status(job_id)?;
        handle.state = JobState::Failed;
        handle.error = Some(error);
        self.write(&handle)?;
        self.append_log(
            &handle.id,
            job_log_entry(&handle, "error", "Job failed", None),
        )?;
        Ok(handle)
    }
}

impl JobRegistry for FileJobRegistry {
    fn register(&self, owner: &str) -> RefineResult<JobHandle> {
        let handle = JobHandle {
            id: new_job_id(),
            owner: owner.to_string(),
            state: JobState::Running,
            progress: empty_object(),
            result: empty_object(),
            error: None,
        };
        self.write(&handle)?;
        self.append_log(
            &handle.id,
            job_log_entry(&handle, "info", "Job registered", None),
        )?;
        Ok(handle)
    }

    fn status(&self, job_id: &str) -> RefineResult<JobHandle> {
        let path = self.job_path(job_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Job {job_id} was not found"));
            }
            RefineError::Io(format!("failed to read job {}: {error}", path.display()))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse job {}: {error}", path.display()))
        })
    }

    fn cancel(&self, job_id: &str) -> RefineResult<JobHandle> {
        let mut handle = self.status(job_id)?;
        handle.state = JobState::Cancelled;
        self.write(&handle)?;
        self.append_log(
            &handle.id,
            job_log_entry(&handle, "warning", "Job cancelled", None),
        )?;
        Ok(handle)
    }

    fn recover(&self) -> RefineResult<Vec<JobHandle>> {
        let dir = self.jobs_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read job registry {}: {error}",
                dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!("failed to inspect job registry entry: {error}"))
            })?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read job {}: {error}",
                    entry.path().display()
                ))
            })?;
            let job = serde_json::from_slice::<JobHandle>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse job {}: {error}",
                    entry.path().display()
                ))
            })?;
            jobs.push(job);
        }
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(jobs)
    }
}

fn new_job_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:x}{:x}{:x}",
        now.as_millis(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn empty_object() -> Value {
    json!({})
}

fn job_log_entry(
    job: &JobHandle,
    severity: &str,
    message: &str,
    details: Option<crate::model::JsonObject>,
) -> LogEntry {
    let mut details = details.unwrap_or_default();
    details.insert("job_id".to_string(), json!(job.id));
    details.insert("owner".to_string(), json!(job.owner));
    details.insert("state".to_string(), json!(job.state));
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "job".to_string(),
        message: message.to_string(),
        details: Some(details),
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        gap_id: job
            .owner
            .strip_prefix("gap:")
            .map(|gap_id| gap_id.to_string()),
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_job_registry_registers_recovers_and_cancels_jobs() {
        let temp_root = unique_temp_dir("jobs");
        let registry = FileJobRegistry::new(temp_root.join("run/8080"));
        let job = registry.register("bulk_update_gaps").unwrap();
        assert_eq!(job.state, JobState::Running);
        assert_eq!(registry.status(&job.id).unwrap().owner, "bulk_update_gaps");
        assert_eq!(registry.recover().unwrap().len(), 1);

        let interrupted = registry.interrupt_active().unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(
            registry.status(&job.id).unwrap().state,
            JobState::Interrupted
        );

        let cancelled = registry.cancel(&job.id).unwrap();
        assert_eq!(cancelled.state, JobState::Cancelled);
        assert_eq!(cancelled.state.as_api_status(), "cancelled");

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
