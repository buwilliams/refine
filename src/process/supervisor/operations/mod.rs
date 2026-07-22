use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::process::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OperationState {
    Pending,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl OperationState {
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
pub struct OperationHandle {
    pub id: String,
    pub owner: String,
    pub state: OperationState,
    #[serde(default = "empty_object")]
    pub request: Value,
    #[serde(default = "empty_object")]
    pub progress: Value,
    #[serde(default = "empty_object")]
    pub result: Value,
    #[serde(default)]
    pub error: Option<Value>,
}

pub trait OperationRegistry {
    fn register(&self, owner: &str) -> RefineResult<OperationHandle>;
    fn status(&self, operation_id: &str) -> RefineResult<OperationHandle>;
    fn cancel(&self, operation_id: &str) -> RefineResult<OperationHandle>;
    fn recover(&self) -> RefineResult<Vec<OperationHandle>>;
}

#[derive(Clone, Debug)]
pub struct FileOperationRegistry {
    pub runtime_root: PathBuf,
}

impl FileOperationRegistry {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn operations_dir(&self) -> PathBuf {
        self.runtime_root.join("operations")
    }

    fn operation_path(&self, operation_id: &str) -> PathBuf {
        self.operations_dir().join(format!("{operation_id}.json"))
    }

    fn log_path(&self, operation_id: &str) -> PathBuf {
        self.operations_dir()
            .join(format!("{operation_id}.logs.jsonl"))
    }

    fn mutation_lock(&self) -> RefineResult<fs::File> {
        fs::create_dir_all(self.operations_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create operation registry {}: {error}",
                self.operations_dir().display()
            ))
        })?;
        let path = self.operations_dir().join(".mutations.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open operation mutation lock {}: {error}",
                    path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock operation registry {}: {error}",
                path.display()
            ))
        })?;
        Ok(file)
    }

    fn write(&self, handle: &OperationHandle) -> RefineResult<()> {
        fs::create_dir_all(self.operations_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create operation registry {}: {error}",
                self.operations_dir().display()
            ))
        })?;
        let path = self.operation_path(&handle.id);
        let encoded = serde_json::to_vec_pretty(handle).map_err(|error| {
            RefineError::Serialization(format!("failed to encode operation: {error}"))
        })?;
        fs::write(&path, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write operation {}: {error}",
                path.display()
            ))
        })
    }

    pub fn interrupt_active(&self) -> RefineResult<Vec<OperationHandle>> {
        let mut interrupted = Vec::new();
        for operation in self.recover()? {
            if matches!(
                operation.state,
                OperationState::Pending | OperationState::Running | OperationState::Cancelling
            ) {
                let lock = self.mutation_lock()?;
                let mut operation = self.status(&operation.id)?;
                if !matches!(
                    operation.state,
                    OperationState::Pending | OperationState::Running | OperationState::Cancelling
                ) {
                    FileExt::unlock(&lock).ok();
                    continue;
                }
                operation.state = OperationState::Interrupted;
                self.write(&operation)?;
                FileExt::unlock(&lock).ok();
                self.append_log(
                    &operation.id,
                    operation_log_entry(&operation, "warning", "Operation interrupted", None),
                )?;
                interrupted.push(operation);
            }
        }
        Ok(interrupted)
    }

    pub fn register_with_request(
        &self,
        owner: &str,
        request: Value,
    ) -> RefineResult<OperationHandle> {
        let handle = OperationHandle {
            id: new_operation_id(),
            owner: owner.to_string(),
            state: OperationState::Running,
            request,
            progress: empty_object(),
            result: empty_object(),
            error: None,
        };
        self.write(&handle)?;
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation registered", None),
        )?;
        Ok(handle)
    }

    pub fn append_log(&self, operation_id: &str, mut entry: LogEntry) -> RefineResult<LogEntry> {
        let operation = self.status(operation_id)?;
        if entry.datetime.trim().is_empty() {
            entry.datetime = now_timestamp();
        }
        if entry.category.trim().is_empty() {
            entry.category = "operation".to_string();
        }
        if entry.actor.is_none() {
            entry.actor = Some("refine".to_string());
        }
        let mut details = entry.details.unwrap_or_default();
        details
            .entry("operation_id".to_string())
            .or_insert_with(|| json!(operation.id));
        details
            .entry("owner".to_string())
            .or_insert_with(|| json!(operation.owner));
        details
            .entry("state".to_string())
            .or_insert_with(|| json!(operation.state));
        entry.details = Some(details);
        fs::create_dir_all(self.operations_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create operation log registry {}: {error}",
                self.operations_dir().display()
            ))
        })?;
        let path = self.log_path(operation_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open operation log {}: {error}",
                    path.display()
                ))
            })?;
        let encoded = serde_json::to_string(&entry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode operation log: {error}"))
        })?;
        writeln!(file, "{encoded}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append operation log {}: {error}",
                path.display()
            ))
        })?;
        Ok(entry)
    }

    pub fn page_logs(
        &self,
        operation_id: &str,
        limit: usize,
        offset: usize,
    ) -> RefineResult<(Vec<LogEntry>, bool, usize)> {
        self.status(operation_id)?;
        let path = self.log_path(operation_id);
        if !path.exists() {
            return Ok((Vec::new(), false, 0));
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open operation log {}: {error}",
                path.display()
            ))
        })?;
        let mut entries = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read operation log {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<LogEntry>(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse operation log {}: {error}",
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

    pub fn finish(
        &self,
        operation_id: &str,
        state: OperationState,
    ) -> RefineResult<OperationHandle> {
        if !matches!(
            state,
            OperationState::Succeeded
                | OperationState::Failed
                | OperationState::Cancelled
                | OperationState::Interrupted
        ) {
            return Err(RefineError::InvalidInput(
                "finished operations must use a terminal state".to_string(),
            ));
        }
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if completion_would_overwrite_cancellation(&handle.state, &state) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        handle.state = state;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation finished", None),
        )?;
        Ok(handle)
    }

    pub fn update_progress(
        &self,
        operation_id: &str,
        progress: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        handle.progress = progress;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        Ok(handle)
    }

    pub fn finish_with_result(
        &self,
        operation_id: &str,
        state: OperationState,
        result: Value,
    ) -> RefineResult<OperationHandle> {
        if !matches!(state, OperationState::Succeeded | OperationState::Failed) {
            return Err(RefineError::InvalidInput(
                "result operations must finish as succeeded or failed".to_string(),
            ));
        }
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if completion_would_overwrite_cancellation(&handle.state, &state) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        handle.state = state;
        handle.result = result;
        handle.error = None;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation finished", None),
        )?;
        Ok(handle)
    }

    pub fn fail_with_error(
        &self,
        operation_id: &str,
        error: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        // Cancellation is the user's authoritative terminal decision. A worker or cleanup path
        // may still discover a real failure after cancellation wins the mutation lock; retain
        // that evidence without rewriting the durable terminal state.
        if !matches!(handle.state, OperationState::Cancelled) {
            handle.state = OperationState::Failed;
        }
        handle.error = Some(error);
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "error", "Operation failed", None),
        )?;
        Ok(handle)
    }
}

impl OperationRegistry for FileOperationRegistry {
    fn register(&self, owner: &str) -> RefineResult<OperationHandle> {
        self.register_with_request(owner, empty_object())
    }

    fn status(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        let path = self.operation_path(operation_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Operation {operation_id} was not found"));
            }
            RefineError::Io(format!(
                "failed to read operation {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse operation {}: {error}",
                path.display()
            ))
        })
    }

    fn cancel(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if operation_terminal(&handle.state) && !matches!(handle.state, OperationState::Interrupted)
        {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        handle.state = OperationState::Cancelled;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "warning", "Operation cancelled", None),
        )?;
        Ok(handle)
    }

    fn recover(&self) -> RefineResult<Vec<OperationHandle>> {
        let dir = self.operations_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut operations = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read operation registry {}: {error}",
                dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect operation registry entry: {error}"
                ))
            })?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read operation {}: {error}",
                    entry.path().display()
                ))
            })?;
            let operation = serde_json::from_slice::<OperationHandle>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse operation {}: {error}",
                    entry.path().display()
                ))
            })?;
            operations.push(operation);
        }
        operations.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(operations)
    }
}

fn operation_terminal(state: &OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded
            | OperationState::Failed
            | OperationState::Cancelled
            | OperationState::Interrupted
    )
}

fn completion_would_overwrite_cancellation(
    current: &OperationState,
    next: &OperationState,
) -> bool {
    matches!(current, OperationState::Cancelled) && !matches!(next, OperationState::Cancelled)
}

fn new_operation_id() -> String {
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

fn operation_log_entry(
    operation: &OperationHandle,
    severity: &str,
    message: &str,
    details: Option<crate::model::JsonObject>,
) -> LogEntry {
    let mut details = details.unwrap_or_default();
    details.insert("operation_id".to_string(), json!(operation.id));
    details.insert("owner".to_string(), json!(operation.owner));
    details.insert("state".to_string(), json!(operation.state));
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "operation".to_string(),
        message: message.to_string(),
        details: Some(details),
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        goal_id: operation
            .owner
            .strip_prefix("goal:")
            .map(|goal_id| goal_id.to_string()),
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_operation_registry_registers_recovers_and_cancels_operations() {
        let temp_root = unique_temp_dir("operations");
        let registry = FileOperationRegistry::new(temp_root.join("run/8080"));
        let operation = registry.register("bulk_update_goals").unwrap();
        assert_eq!(operation.state, OperationState::Running);
        assert_eq!(
            registry.status(&operation.id).unwrap().owner,
            "bulk_update_goals"
        );
        assert_eq!(registry.recover().unwrap().len(), 1);

        let interrupted = registry.interrupt_active().unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(
            registry.status(&operation.id).unwrap().state,
            OperationState::Interrupted
        );

        let cancelled = registry.cancel(&operation.id).unwrap();
        assert_eq!(cancelled.state, OperationState::Cancelled);
        assert_eq!(cancelled.state.as_api_status(), "cancelled");

        let recovery_failure = registry.register("import:persist").unwrap();
        registry.cancel(&recovery_failure.id).unwrap();
        let failed = registry
            .fail_with_error(
                &recovery_failure.id,
                json!({
                    "code": "projection_refresh_failed",
                    "message": "cancel rollback could not refresh the projection"
                }),
            )
            .unwrap();
        assert_eq!(failed.state, OperationState::Cancelled);
        assert_eq!(failed.error.unwrap()["code"], "projection_refresh_failed");
        assert_eq!(
            registry.status(&recovery_failure.id).unwrap().state,
            OperationState::Cancelled
        );
        let (logs, _, _) = registry.page_logs(&recovery_failure.id, 20, 0).unwrap();
        assert!(logs.iter().any(|entry| entry.message == "Operation failed"));

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
