use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcess};
use crate::process::supervisor::coordination::replace_file_durably;
use crate::process::supervisor::errors::{RefineError, RefineResult};

const RECOVERY_PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplacementOperationRegistration {
    pub operation: OperationHandle,
    pub created: bool,
}

pub trait OperationRegistry {
    fn register(&self, owner: &str) -> RefineResult<OperationHandle>;
    fn status(&self, operation_id: &str) -> RefineResult<OperationHandle>;
    fn cancel(&self, operation_id: &str) -> RefineResult<OperationHandle>;
    fn recover(&self) -> RefineResult<Vec<OperationHandle>>;
}

pub trait OperationProjectionRefresher {
    fn refresh_operation_projection(&self) -> RefineResult<()>;
}

impl<F> OperationProjectionRefresher for F
where
    F: Fn() -> RefineResult<()>,
{
    fn refresh_operation_projection(&self) -> RefineResult<()> {
        self()
    }
}

#[derive(Clone, Debug)]
pub struct FileOperationRegistry {
    pub runtime_root: PathBuf,
}

/// Holds the same mutation lock used by cancellation until a supervised process has been
/// durably registered. This closes the gap where cancellation could observe no process and a
/// worker could launch one immediately afterward.
pub struct OperationLaunchGuard {
    _lock: fs::File,
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

    fn workflow_cancellation_path(&self, execution_id: &str) -> PathBuf {
        let encoded = execution_id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.operations_dir()
            .join(".workflow-cancellations")
            .join(format!("{encoded}.json"))
    }

    fn workflow_cancellation(&self, execution_id: &str) -> RefineResult<Option<Value>> {
        let path = self.workflow_cancellation_path(execution_id);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse workflow cancellation {}: {error}",
                    path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to read workflow cancellation {}: {error}",
                path.display()
            ))),
        }
    }

    fn ensure_request_execution_active(&self, request: &Value) -> RefineResult<()> {
        let Some(execution_id) = request
            .get("execution_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|execution_id| !execution_id.is_empty())
        else {
            return Ok(());
        };
        if self.workflow_cancellation(execution_id)?.is_some() {
            return Err(RefineError::Conflict(format!(
                "Workflow execution {execution_id} was cancelled before operation registration"
            )));
        }
        Ok(())
    }

    fn persist_workflow_cancellation(&self, execution_id: &str) -> RefineResult<Value> {
        let cancellation = json!({
            "execution_id": execution_id,
            "cancelled_at": now_timestamp(),
            "error": {
                "code": "workflow_execution_cancelled",
                "message": format!(
                    "Workflow execution {execution_id} was cancelled before or while an operation was active"
                ),
                "execution_id": execution_id
            }
        });
        let encoded = serde_json::to_vec_pretty(&cancellation).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode workflow cancellation for {execution_id}: {error}"
            ))
        })?;
        replace_file_durably(&self.workflow_cancellation_path(execution_id), &encoded)?;
        Ok(cancellation)
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

    pub fn active_launch_guard(&self, operation_id: &str) -> RefineResult<OperationLaunchGuard> {
        let lock = self.mutation_lock()?;
        let operation = self.status(operation_id)?;
        if !matches!(
            operation.state,
            OperationState::Pending | OperationState::Running
        ) {
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {operation_id} is {}; no later supervised process may start",
                operation.state.as_api_status()
            )));
        }
        Ok(OperationLaunchGuard { _lock: lock })
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
        let temp_path = path.with_extension(format!("json.{}.tmp", std::process::id()));
        fs::write(&temp_path, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write temporary operation {}: {error}",
                temp_path.display()
            ))
        })?;
        if let Err(error) = replace_file(&temp_path, &path) {
            let _ = fs::remove_file(&temp_path);
            return Err(RefineError::Io(format!(
                "failed to replace operation {}: {error}",
                path.display()
            )));
        }
        Ok(())
    }

    pub fn interrupt_active(&self) -> RefineResult<Vec<OperationHandle>> {
        Ok(self
            .recover_active_supervised()?
            .into_iter()
            .filter(|operation| matches!(operation.state, OperationState::Interrupted))
            .collect())
    }

    /// Reconciles active operations during daemon startup. Every correlated managed process must
    /// be confirmed dead before the operation becomes Interrupted and therefore publicly
    /// retryable. If termination cannot be confirmed, the operation becomes a durable Failed
    /// attention state while retaining its request, progress, result, and existing logs.
    pub fn recover_active_supervised(&self) -> RefineResult<Vec<OperationHandle>> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        let processes = supervisor.list()?;
        let mut recovered = Vec::new();

        for operation in self.recover()? {
            if !operation_active(&operation.state) {
                continue;
            }
            let deferred_cancellation = matches!(operation.state, OperationState::Cancelling)
                && cancellation_terminal_is_deferred(&operation);
            let Some(operation) = self.begin_recovery(&operation.id)? else {
                continue;
            };
            let associated = processes
                .iter()
                .filter(|process| {
                    process_operation_id(process).as_deref() == Some(operation.id.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            match self.terminate_recovery_processes(&supervisor, &operation, &associated) {
                Ok(()) => {
                    if deferred_cancellation {
                        // The owning capability must durably persist its cancellation evidence
                        // before this operation becomes terminal. Keep the launch-blocking state
                        // recoverable for that capability's startup reconciliation.
                        recovered.push(self.status(&operation.id)?);
                    } else if let Some(interrupted) = self.interrupt_if_active(&operation.id)? {
                        recovered.push(interrupted);
                    }
                }
                Err(error) => {
                    if deferred_cancellation {
                        self.record_recoverable_failure(
                            &operation.id,
                            "operation_recovery_process_termination_failed",
                            &error,
                        )?;
                        recovered.push(self.status(&operation.id)?);
                    } else if let Some(failed) =
                        self.fail_recovery_if_active(&operation, &associated, &error)?
                    {
                        recovered.push(failed);
                    }
                }
            }
        }
        Ok(recovered)
    }

    fn begin_recovery(&self, operation_id: &str) -> RefineResult<Option<OperationHandle>> {
        let lock = self.mutation_lock()?;
        let mut operation = self.status(operation_id)?;
        if !operation_active(&operation.state) {
            FileExt::unlock(&lock).ok();
            return Ok(None);
        }
        operation.state = OperationState::Cancelling;
        self.write(&operation)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            operation_id,
            operation_log_entry(
                &operation,
                "warning",
                "Operation restart recovery started",
                None,
            ),
        )?;
        Ok(Some(operation))
    }

    fn terminate_recovery_processes(
        &self,
        supervisor: &FileProcessSupervisor,
        operation: &OperationHandle,
        processes: &[ManagedProcess],
    ) -> RefineResult<()> {
        #[cfg(test)]
        if operation
            .request
            .get("test_inject_recovery_termination_failure")
            .and_then(Value::as_bool)
            == Some(true)
            && !processes.is_empty()
        {
            return Err(RefineError::Degraded(
                "injected managed-process termination failure".to_string(),
            ));
        }
        for process in processes {
            self.append_log(
                &operation.id,
                operation_log_entry(
                    operation,
                    "warning",
                    "Recovery terminating managed process",
                    Some(crate::model::JsonObject::from_iter([
                        ("process_id".to_string(), json!(process.id)),
                        ("pid".to_string(), json!(process.pid)),
                    ])),
                ),
            )?;
            supervisor.terminate_and_confirm_exit(process, RECOVERY_PROCESS_EXIT_TIMEOUT)?;
            self.append_log(
                &operation.id,
                operation_log_entry(
                    operation,
                    "info",
                    "Recovery confirmed managed process exit",
                    Some(crate::model::JsonObject::from_iter([
                        ("process_id".to_string(), json!(process.id)),
                        ("pid".to_string(), json!(process.pid)),
                    ])),
                ),
            )?;
        }
        Ok(())
    }

    fn interrupt_if_active(&self, operation_id: &str) -> RefineResult<Option<OperationHandle>> {
        let lock = self.mutation_lock()?;
        let mut operation = self.status(operation_id)?;
        if !operation_active(&operation.state) {
            FileExt::unlock(&lock).ok();
            return Ok(None);
        }
        operation.state = OperationState::Interrupted;
        operation.error = Some(json!({
            "code": "operation_interrupted",
            "message": "Daemon restarted before the operation completed."
        }));
        self.write(&operation)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &operation.id,
            operation_log_entry(
                &operation,
                "warning",
                "Operation interrupted",
                Some(crate::model::JsonObject::from_iter([(
                    "reason".to_string(),
                    json!("daemon_restart"),
                )])),
            ),
        )?;
        Ok(Some(operation))
    }

    fn fail_recovery_if_active(
        &self,
        operation: &OperationHandle,
        processes: &[ManagedProcess],
        error: &RefineError,
    ) -> RefineResult<Option<OperationHandle>> {
        let lock = self.mutation_lock()?;
        let mut current = self.status(&operation.id)?;
        if !operation_active(&current.state) {
            FileExt::unlock(&lock).ok();
            return Ok(None);
        }
        current.state = OperationState::Failed;
        current.error = Some(json!({
            "code": "operation_recovery_process_termination_failed",
            "message": error.to_string(),
            "attention_required": true,
            "retryable": false,
            "processes": processes.iter().map(|process| json!({
                "id": process.id,
                "pid": process.pid
            })).collect::<Vec<_>>(),
            "previous_error": operation.error
        }));
        self.write(&current)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &current.id,
            operation_log_entry(
                &current,
                "error",
                "Recovery could not confirm managed process exit",
                Some(crate::model::JsonObject::from_iter([
                    ("error".to_string(), json!(error.to_string())),
                    ("retryable".to_string(), json!(false)),
                ])),
            ),
        )?;
        Ok(Some(current))
    }

    pub fn register_with_request(
        &self,
        owner: &str,
        request: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        self.ensure_request_execution_active(&request)?;
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
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation registered", None),
        )?;
        Ok(handle)
    }

    /// Atomically registers one active operation for an ownership key.
    ///
    /// Quality uses a Goal-and-candidate ownership key so manual and workflow callers cannot
    /// evaluate or mutate the same candidate concurrently. The operation registry mutation lock
    /// makes the exclusion effective across threads and daemon processes.
    pub fn register_exclusive_with_request(
        &self,
        owner: &str,
        request: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        self.ensure_request_execution_active(&request)?;
        if let Some(active) = self
            .recover()?
            .into_iter()
            .find(|operation| operation.owner == owner && operation_active(&operation.state))
        {
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {} already owns {owner}",
                active.id
            )));
        }
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
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "info", "Operation registered", None),
        )?;
        Ok(handle)
    }

    /// Atomically claims a durable replacement for an interrupted operation.
    ///
    /// The source id and retry identity are written into the replacement request while the
    /// registry mutation lock is held. Concurrent callers, including callers in separate daemon
    /// processes, therefore either create one replacement or recover that exact same record.
    pub fn find_or_register_replacement(
        &self,
        owner: &str,
        source_operation_id: &str,
        retry_identity: &str,
        mut request: Value,
    ) -> RefineResult<ReplacementOperationRegistration> {
        if retry_identity.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "Replacement operation retry identity is required".to_string(),
            ));
        }

        let lock = self.mutation_lock()?;
        let source = self.status(source_operation_id)?;
        if source.owner != owner || !matches!(source.state, OperationState::Interrupted) {
            return Err(RefineError::Conflict(format!(
                "Only interrupted {owner} operations can be recovered"
            )));
        }

        let request_object = request.as_object_mut().ok_or_else(|| {
            RefineError::InvalidInput(
                "Replacement operation request must be a JSON object".to_string(),
            )
        })?;
        request_object.insert("recovery_of".to_string(), json!(source_operation_id));
        request_object.insert("retry_identity".to_string(), json!(retry_identity));

        let mut legacy_replacement = None;
        for mut operation in self.recover()? {
            let operation_retry_identity = operation
                .request
                .get("retry_identity")
                .and_then(Value::as_str);
            let operation_source = operation.request.get("recovery_of").and_then(Value::as_str);

            if operation_retry_identity == Some(retry_identity) {
                if operation.owner != owner || operation_source != Some(source_operation_id) {
                    return Err(RefineError::Conflict(format!(
                        "Retry identity {retry_identity} is already assigned to another operation"
                    )));
                }
                drop(lock);
                return Ok(ReplacementOperationRegistration {
                    operation,
                    created: false,
                });
            }

            // Round 6 replacements predate the explicit retry identity. Adopt that durable record
            // instead of creating a duplicate after an upgrade or restart.
            if operation.owner == owner
                && operation_source == Some(source_operation_id)
                && operation_retry_identity.is_none()
            {
                let operation_request = operation.request.as_object_mut().ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "replacement operation {} request is not a JSON object",
                        operation.id
                    ))
                })?;
                operation_request.insert("retry_identity".to_string(), json!(retry_identity));
                legacy_replacement = Some(operation);
                break;
            }
        }

        if let Some(operation) = legacy_replacement {
            self.write(&operation)?;
            drop(lock);
            return Ok(ReplacementOperationRegistration {
                operation,
                created: false,
            });
        }

        let operation = OperationHandle {
            id: new_operation_id(),
            owner: owner.to_string(),
            state: OperationState::Running,
            request,
            progress: empty_object(),
            result: empty_object(),
            error: None,
        };
        self.write(&operation)?;
        self.append_log(
            &operation.id,
            operation_log_entry(&operation, "info", "Operation registered", None),
        )?;
        drop(lock);
        Ok(ReplacementOperationRegistration {
            operation,
            created: true,
        })
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

    /// Cancels an operation and all managed processes associated with it, then refreshes the
    /// caller's projection of runtime state. Durable cancellation is written first so a worker
    /// racing with termination cannot publish success over the user's cancellation.
    ///
    /// The projection refresher keeps projection construction independent of the operation
    /// registry while this shared capability remains responsible for sequencing it and persisting
    /// any failure evidence.
    pub fn cancel_supervised(
        &self,
        operation_id: &str,
        projection_refresher: &impl OperationProjectionRefresher,
    ) -> RefineResult<OperationHandle> {
        let operation = self.cancel(operation_id)?;

        if let Err(error) = self.terminate_associated_processes(operation_id) {
            self.persist_cancellation_failure(
                operation_id,
                "operation_process_termination_failed",
                &error,
            )?;
            return Err(error);
        }
        if let Err(error) = projection_refresher.refresh_operation_projection() {
            self.persist_cancellation_failure(
                operation_id,
                "operation_cancel_projection_refresh_failed",
                &error,
            )?;
            return Err(error);
        }

        Ok(operation)
    }

    /// Cancels every active operation durably owned by one workflow execution.
    ///
    /// Ready Merge uses this before changing the workflow claim. Operation cancellation and
    /// registration share the mutation lock. A durable execution tombstone is written even when
    /// no operation exists yet, so a worker that already holds workflow authority cannot register
    /// and launch after cancellation's first scan. Repeating the call remains intentional because
    /// the workflow coordination lock orders the tombstone with the authoritative claim update.
    pub fn cancel_workflow_execution_operations(
        &self,
        execution_id: &str,
    ) -> RefineResult<Vec<OperationHandle>> {
        let execution_id = execution_id.trim();
        if execution_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "Workflow execution id is required for cancellation".to_string(),
            ));
        }
        let lock = self.mutation_lock()?;
        let cancellation = self.persist_workflow_cancellation(execution_id)?;
        let error = cancellation.get("error").cloned().unwrap_or_else(|| {
            json!({
                "code": "workflow_execution_cancelled",
                "message": format!("Workflow execution {execution_id} was cancelled"),
                "execution_id": execution_id
            })
        });
        let mut cancelled = self
            .recover()?
            .into_iter()
            .filter(|operation| {
                operation
                    .request
                    .get("execution_id")
                    .and_then(Value::as_str)
                    == Some(execution_id)
                    && operation_active(&operation.state)
            })
            .collect::<Vec<_>>();
        for operation in &mut cancelled {
            operation.state = OperationState::Cancelled;
            operation.error = Some(error.clone());
            self.write(operation)?;
        }
        FileExt::unlock(&lock).ok();

        for operation in &cancelled {
            self.append_log(
                &operation.id,
                operation_log_entry(operation, "warning", "Workflow operation cancelled", None),
            )?;
            if let Err(error) = self.terminate_associated_processes(&operation.id) {
                self.persist_cancellation_failure(
                    &operation.id,
                    "operation_process_termination_failed",
                    &error,
                )?;
                return Err(error);
            }
        }
        Ok(cancelled)
    }

    fn terminate_associated_processes(&self, operation_id: &str) -> RefineResult<()> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        for process in supervisor
            .list()?
            .iter()
            .filter(|process| process_operation_id(process).as_deref() == Some(operation_id))
        {
            supervisor.request_termination(&process.id, "terminate")?;
        }
        Ok(())
    }

    fn persist_cancellation_failure(
        &self,
        operation_id: &str,
        code: &str,
        error: &RefineError,
    ) -> RefineResult<()> {
        self.fail_with_error(
            operation_id,
            json!({
                "code": code,
                "message": error.to_string()
            }),
        )?;
        Ok(())
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
        if terminal_recovery_state_is_authoritative(&handle.state, &state) {
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

    /// Completes a capability-owned two-phase cancellation after its durable evidence is stored.
    /// Repeated settlement is idempotent, while unrelated operation states cannot be converted to
    /// cancelled accidentally.
    pub fn settle_cancellation(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if matches!(handle.state, OperationState::Cancelled) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        if !matches!(handle.state, OperationState::Cancelling) {
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {operation_id} is {}; only cancelling operations can settle as cancelled",
                handle.state.as_api_status()
            )));
        }
        let live = self.live_owned_processes(operation_id)?;
        if !live.is_empty() {
            let error = RefineError::Degraded(format!(
                "Quality cancellation cannot settle while {} owned managed process(es) remain alive",
                live.len()
            ));
            if handle
                .error
                .as_ref()
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
                != Some("operation_recovery_process_termination_failed")
            {
                handle.error = Some(json!({
                    "code": "operation_cancellation_process_still_alive",
                    "message": error.to_string(),
                    "attention_required": true,
                    "retryable": true,
                    "processes": live
                }));
            }
            self.write(&handle)?;
            FileExt::unlock(&lock).ok();
            self.append_log(
                &handle.id,
                operation_log_entry(
                    &handle,
                    "error",
                    "Cancellation settlement deferred until managed processes exit",
                    Some(crate::model::JsonObject::from_iter([(
                        "error".to_string(),
                        json!(error.to_string()),
                    )])),
                ),
            )?;
            return Err(error);
        }
        handle.state = OperationState::Cancelled;
        handle.error = None;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(&handle, "warning", "Operation cancelled", None),
        )?;
        Ok(handle)
    }

    /// Verifies the process half of a deferred cancellation before capability evidence is made
    /// terminal. Cancellation and supervised launch share the operation mutation barrier, so no
    /// later operation-owned launch can appear after this returns successfully.
    pub fn ensure_cancellation_processes_exited(
        &self,
        operation_id: &str,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let handle = self.status(operation_id)?;
        if matches!(handle.state, OperationState::Cancelled) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        if !matches!(handle.state, OperationState::Cancelling) {
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {operation_id} is {}; only cancelling operations can verify process exit",
                handle.state.as_api_status()
            )));
        }
        let live = self.live_owned_processes(operation_id)?;
        FileExt::unlock(&lock).ok();
        if live.is_empty() {
            Ok(handle)
        } else {
            Err(RefineError::Degraded(format!(
                "Quality cancellation cannot persist terminal evidence while {} owned managed process(es) remain alive",
                live.len()
            )))
        }
    }

    fn live_owned_processes(&self, operation_id: &str) -> RefineResult<Vec<Value>> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        let mut live = Vec::new();
        for process in supervisor
            .list()?
            .into_iter()
            .filter(|process| process_operation_id(process).as_deref() == Some(operation_id))
        {
            if FileProcessSupervisor::process_is_alive(&process)? {
                live.push(json!({"id": process.id, "pid": process.pid}));
            }
        }
        Ok(live)
    }

    /// Records a startup/capability recovery failure without making a deferred cancellation
    /// terminal. A later daemon start can retry after the underlying process or state store is
    /// available again.
    pub fn record_recoverable_failure(
        &self,
        operation_id: &str,
        code: &str,
        error: &RefineError,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if !matches!(handle.state, OperationState::Cancelling) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        let preserve_termination_failure = handle
            .error
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            == Some("operation_recovery_process_termination_failed")
            && code != "operation_recovery_process_termination_failed";
        if !preserve_termination_failure {
            handle.error = Some(json!({
                "code": code,
                "message": error.to_string(),
                "attention_required": true,
                "retryable": true
            }));
        }
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(
                &handle,
                "error",
                "Deferred cancellation recovery remains incomplete",
                Some(crate::model::JsonObject::from_iter([
                    ("code".to_string(), json!(code)),
                    ("error".to_string(), json!(error.to_string())),
                ])),
            ),
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
        // A restart-interrupted worker may still race with recovery after its process is
        // terminated. Preserve the progress snapshot that was captured at interruption so the
        // UI cannot regress to a misleading completed state. Other terminal operations retain
        // their established progress-update behavior (for example, import cancellation records
        // its final acknowledgement after cancellation wins the state race).
        if matches!(handle.state, OperationState::Interrupted) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        handle.progress = progress;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        Ok(handle)
    }

    pub fn succeed_with_result_and_progress(
        &self,
        operation_id: &str,
        progress: Value,
        result: Value,
    ) -> RefineResult<OperationHandle> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if operation_terminal(&handle.state) {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        handle.state = OperationState::Succeeded;
        handle.progress = progress;
        handle.result = result;
        handle.error = None;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        Ok(handle)
    }

    /// Runs the capability's final state transition and operation settlement under the same
    /// mutation lock used by cancellation.
    ///
    /// If cancellation wins the lock, the transition is never invoked. If settlement wins, the
    /// transition and success record become one ordered decision and a later cancellation cannot
    /// replace them.
    pub fn succeed_after<T>(
        &self,
        operation_id: &str,
        progress: Value,
        result: Value,
        transition: impl FnOnce() -> RefineResult<T>,
    ) -> RefineResult<(OperationHandle, T)> {
        let lock = self.mutation_lock()?;
        let mut handle = self.status(operation_id)?;
        if !matches!(
            handle.state,
            OperationState::Pending | OperationState::Running
        ) {
            let state = handle.state.as_api_status();
            FileExt::unlock(&lock).ok();
            return Err(RefineError::Conflict(format!(
                "Operation {operation_id} is {state}; workflow settlement no longer owns it"
            )));
        }
        let transitioned = match transition() {
            Ok(transitioned) => transitioned,
            Err(error) => {
                FileExt::unlock(&lock).ok();
                return Err(error);
            }
        };
        handle.state = OperationState::Succeeded;
        handle.progress = progress;
        handle.result = result;
        handle.error = None;
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        Ok((handle, transitioned))
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
        if terminal_recovery_state_is_authoritative(&handle.state, &state) {
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
        // Cancellation and restart interruption are authoritative terminal decisions. A worker or
        // cleanup path may still discover a real failure after either wins the mutation lock.
        let recovery_owned = matches!(
            handle.state,
            OperationState::Cancelling | OperationState::Interrupted
        );
        if !matches!(
            handle.state,
            OperationState::Cancelling | OperationState::Cancelled | OperationState::Interrupted
        ) {
            handle.state = OperationState::Failed;
        }
        if !recovery_owned {
            handle.error = Some(error.clone());
        }
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(
                &handle,
                "error",
                "Operation failed",
                Some(crate::model::JsonObject::from_iter([(
                    "error".to_string(),
                    error,
                )])),
            ),
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
        if matches!(handle.state, OperationState::Cancelling)
            && cancellation_terminal_is_deferred(&handle)
        {
            FileExt::unlock(&lock).ok();
            return Ok(handle);
        }
        let deferred = handle
            .request
            .get("defer_cancellation_terminal")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if deferred && let Some(request) = handle.request.as_object_mut() {
            request.insert("cancellation_requested".to_string(), json!(true));
        }
        handle.state = if deferred {
            OperationState::Cancelling
        } else {
            OperationState::Cancelled
        };
        self.write(&handle)?;
        FileExt::unlock(&lock).ok();
        self.append_log(
            &handle.id,
            operation_log_entry(
                &handle,
                "warning",
                if deferred {
                    "Operation cancellation requested"
                } else {
                    "Operation cancelled"
                },
                None,
            ),
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

fn operation_active(state: &OperationState) -> bool {
    matches!(
        state,
        OperationState::Pending | OperationState::Running | OperationState::Cancelling
    )
}

fn cancellation_terminal_is_deferred(operation: &OperationHandle) -> bool {
    operation
        .request
        .get("defer_cancellation_terminal")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && operation
            .request
            .get("cancellation_requested")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

#[cfg(not(windows))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn terminal_recovery_state_is_authoritative(
    current: &OperationState,
    next: &OperationState,
) -> bool {
    matches!(
        current,
        OperationState::Cancelling | OperationState::Cancelled | OperationState::Interrupted
    ) && current != next
}

fn process_operation_id(process: &ManagedProcess) -> Option<String> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| {
            details
                .get("operation_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
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
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn exclusive_operation_registration_serializes_one_owner() {
        let temp_root = unique_temp_dir("operations-exclusive-owner");
        let registry = FileOperationRegistry::new(&temp_root);
        let first = registry
            .register_exclusive_with_request("quality:GOAL1:abc", json!({"source": "workflow"}))
            .unwrap();
        let conflict = registry
            .register_exclusive_with_request("quality:GOAL1:abc", json!({"source": "manual"}))
            .unwrap_err();
        assert!(conflict.to_string().contains(&first.id));
        registry
            .finish(&first.id, OperationState::Succeeded)
            .unwrap();
        assert!(
            registry
                .register_exclusive_with_request("quality:GOAL1:abc", json!({"source": "manual"}))
                .is_ok()
        );
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn workflow_cancellation_tombstone_blocks_late_operation_registration() {
        let temp_root = unique_temp_dir("operations-workflow-cancellation-tombstone");
        let registry = FileOperationRegistry::new(&temp_root);

        assert!(
            registry
                .cancel_workflow_execution_operations("execution-cancelled-before-registration")
                .unwrap()
                .is_empty()
        );
        let exclusive = registry
            .register_exclusive_with_request(
                "merger:GOAL1:1",
                json!({"execution_id": "execution-cancelled-before-registration"}),
            )
            .unwrap_err();
        assert!(
            exclusive
                .to_string()
                .contains("cancelled before operation registration"),
            "{exclusive}"
        );
        let ordinary = registry
            .register_with_request(
                "workflow:test",
                json!({"execution_id": "execution-cancelled-before-registration"}),
            )
            .unwrap_err();
        assert!(
            ordinary
                .to_string()
                .contains("cancelled before operation registration"),
            "{ordinary}"
        );
        assert!(
            registry
                .register_exclusive_with_request(
                    "merger:GOAL1:1",
                    json!({"execution_id": "replacement-execution"}),
                )
                .is_ok()
        );

        let cancellations = fs::read_dir(registry.operations_dir().join(".workflow-cancellations"))
            .unwrap()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        assert_eq!(cancellations.len(), 1);
        let evidence: Value =
            serde_json::from_slice(&fs::read(cancellations[0].path()).unwrap()).unwrap();
        assert_eq!(evidence["error"]["code"], "workflow_execution_cancelled");
        assert!(
            evidence["error"]["message"]
                .as_str()
                .is_some_and(|message| !message.trim().is_empty())
        );
        fs::remove_dir_all(temp_root).unwrap();
    }

    use crate::process::subprocess::{
        ManagedProcessSpec, ProcessOwner, ProcessResourceLimits, ProcessSupervisor,
        managed_pid_is_alive,
    };

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
        assert_eq!(
            registry.status(&operation.id).unwrap().error.unwrap()["code"],
            "operation_interrupted"
        );
        let late_completion = registry
            .finish_with_result(
                &operation.id,
                OperationState::Succeeded,
                json!({"result": "must not replace interruption"}),
            )
            .unwrap();
        assert_eq!(late_completion.state, OperationState::Interrupted);
        let late_failure = registry
            .fail_with_error(
                &operation.id,
                json!({"code": "late_worker_failure", "message": "worker exited late"}),
            )
            .unwrap();
        assert_eq!(late_failure.state, OperationState::Interrupted);
        assert_eq!(late_failure.error.unwrap()["code"], "operation_interrupted");
        let late_progress = registry
            .update_progress(&operation.id, json!({"stage": "complete"}))
            .unwrap();
        assert_eq!(late_progress.state, OperationState::Interrupted);
        assert_eq!(late_progress.progress, json!({}));

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

    #[test]
    fn file_operation_registry_state_replacement_never_exposes_partial_json() {
        let temp_root = unique_temp_dir("operations-atomic-state");
        let registry = FileOperationRegistry::new(temp_root.join("run/8080"));
        let operation = registry.register("goals:jira-export").unwrap();
        let reader_registry = registry.clone();
        let operation_id = operation.id.clone();
        let reader = thread::spawn(move || {
            for _ in 0..500 {
                reader_registry.status(&operation_id).unwrap();
            }
        });
        for completed in 0..500 {
            registry
                .update_progress(&operation.id, json!({"completed": completed}))
                .unwrap();
        }
        reader.join().unwrap();
        assert_eq!(
            registry.status(&operation.id).unwrap().progress["completed"],
            499
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_operation_registry_atomically_finds_or_registers_one_durable_replacement() {
        let temp_root = unique_temp_dir("operations-replacement-idempotency");
        let runtime_root = temp_root.join("run/8080");
        let registry = FileOperationRegistry::new(&runtime_root);
        let source = registry
            .register_with_request(
                "goals:jira-export",
                json!({"selection": {"selected_ids": ["GOAL1"]}}),
            )
            .unwrap();
        registry.interrupt_active().unwrap();

        let retry_identity = format!("goals:jira-export:retry:{}", source.id);
        let barrier = Arc::new(Barrier::new(3));
        let callers = (0..2)
            .map(|_| {
                let registry = registry.clone();
                let source_id = source.id.clone();
                let retry_identity = retry_identity.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    registry
                        .find_or_register_replacement(
                            "goals:jira-export",
                            &source_id,
                            &retry_identity,
                            json!({"selection": {"selected_ids": ["GOAL1"]}}),
                        )
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let registrations = callers
            .into_iter()
            .map(|caller| caller.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(registrations[0].operation.id, registrations[1].operation.id);
        assert_eq!(
            registrations
                .iter()
                .filter(|registration| registration.created)
                .count(),
            1
        );
        let replacement = &registrations[0].operation;
        assert_eq!(replacement.request["recovery_of"], source.id);
        assert_eq!(replacement.request["retry_identity"], retry_identity);
        assert_eq!(
            registry
                .recover()
                .unwrap()
                .iter()
                .filter(|operation| operation.request["recovery_of"] == source.id)
                .count(),
            1
        );

        let reopened = FileOperationRegistry::new(&runtime_root)
            .find_or_register_replacement(
                "goals:jira-export",
                &source.id,
                &retry_identity,
                json!({"selection": {"selected_ids": ["GOAL1"]}}),
            )
            .unwrap();
        assert!(!reopened.created);
        assert_eq!(reopened.operation.id, replacement.id);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_operation_registry_supervised_cancel_terminates_process_and_refreshes_projection() {
        let temp_root = unique_temp_dir("operations-supervised-cancel");
        let runtime_root = temp_root.join("run/8080");
        let registry = FileOperationRegistry::new(&runtime_root);
        let operation = registry.register("goals:jira-export").unwrap();
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let process = supervisor
            .launch(operation_helper_process_spec(&operation.id))
            .unwrap();
        let pid = process.pid.unwrap();
        assert!(managed_pid_is_alive(pid).unwrap());

        let projection_refreshed = AtomicBool::new(false);
        let projection_refresher = || {
            projection_refreshed.store(true, AtomicOrdering::SeqCst);
            Ok(())
        };
        let cancelled = registry
            .cancel_supervised(&operation.id, &projection_refresher)
            .unwrap();

        assert_eq!(cancelled.state, OperationState::Cancelled);
        assert!(projection_refreshed.load(AtomicOrdering::SeqCst));
        wait_for_managed_pid_exit(pid);
        assert!(!managed_pid_is_alive(pid).unwrap());
        assert_eq!(
            registry.status(&operation.id).unwrap().state,
            OperationState::Cancelled
        );
        let late_completion = registry
            .finish_with_result(
                &operation.id,
                OperationState::Succeeded,
                json!({"result": "must not replace cancellation"}),
            )
            .unwrap();
        assert_eq!(late_completion.state, OperationState::Cancelled);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn operation_launch_guard_serializes_cancel_and_rejects_late_launch() {
        let temp_root = unique_temp_dir("operations-launch-cancel-barrier");
        let runtime_root = temp_root.join("run/8080");
        let registry = FileOperationRegistry::new(&runtime_root);
        let operation = registry.register("quality:GOAL1:abc").unwrap();

        let launch_guard = registry.active_launch_guard(&operation.id).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let cancel_registry = registry.clone();
        let cancel_operation_id = operation.id.clone();
        let cancel_barrier = Arc::clone(&barrier);
        let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
        let cancellation = thread::spawn(move || {
            cancel_barrier.wait();
            let cancelled = cancel_registry.cancel(&cancel_operation_id).unwrap();
            cancelled_tx.send(cancelled).unwrap();
        });
        barrier.wait();
        assert!(
            cancelled_rx
                .recv_timeout(Duration::from_millis(25))
                .is_err(),
            "cancellation must wait until process registration releases the launch barrier"
        );
        drop(launch_guard);
        assert_eq!(
            cancelled_rx.recv().unwrap().state,
            OperationState::Cancelled
        );
        cancellation.join().unwrap();

        let late_launch = FileProcessSupervisor::new(&runtime_root)
            .launch(operation_helper_process_spec(&operation.id))
            .unwrap_err();
        assert!(
            late_launch
                .to_string()
                .contains("no later supervised process may start")
        );
        assert!(
            FileProcessSupervisor::new(&runtime_root)
                .list()
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_operation_registry_supervised_cancel_persists_capability_failures() {
        let temp_root = unique_temp_dir("operations-supervised-cancel-failures");

        let termination_runtime_root = temp_root.join("run/termination");
        let termination_registry = FileOperationRegistry::new(&termination_runtime_root);
        let termination_operation = termination_registry.register("goals:jira-export").unwrap();
        fs::write(
            termination_runtime_root.join("processes"),
            b"not a directory",
        )
        .unwrap();
        let projection_refreshed = AtomicBool::new(false);
        let projection_refresher = || {
            projection_refreshed.store(true, AtomicOrdering::SeqCst);
            Ok(())
        };
        let termination_error = termination_registry
            .cancel_supervised(&termination_operation.id, &projection_refresher)
            .unwrap_err();
        assert!(termination_error.to_string().contains("process registry"));
        assert!(!projection_refreshed.load(AtomicOrdering::SeqCst));
        let termination_operation = termination_registry
            .status(&termination_operation.id)
            .unwrap();
        assert_eq!(termination_operation.state, OperationState::Cancelled);
        assert_eq!(
            termination_operation.error.unwrap()["code"],
            "operation_process_termination_failed"
        );

        let projection_runtime_root = temp_root.join("run/projection");
        let projection_registry = FileOperationRegistry::new(&projection_runtime_root);
        let projection_operation = projection_registry.register("goals:jira-export").unwrap();
        let projection_refresher = || Err(RefineError::Io("projection refresh failed".to_string()));
        let projection_error = projection_registry
            .cancel_supervised(&projection_operation.id, &projection_refresher)
            .unwrap_err();
        assert_eq!(projection_error.to_string(), "projection refresh failed");
        let projection_operation = projection_registry
            .status(&projection_operation.id)
            .unwrap();
        assert_eq!(projection_operation.state, OperationState::Cancelled);
        assert_eq!(
            projection_operation.error.unwrap()["code"],
            "operation_cancel_projection_refresh_failed"
        );
        let (logs, _, _) = projection_registry
            .page_logs(&projection_operation.id, 20, 0)
            .unwrap();
        assert!(logs.iter().any(|entry| entry.message == "Operation failed"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn operation_helper_process_spec(operation_id: &str) -> ManagedProcessSpec {
        #[cfg(windows)]
        let (command, args) = (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()],
        );
        #[cfg(not(windows))]
        let (command, args) = (
            "sh".to_string(),
            vec!["-c".to_string(), "while :; do sleep 1; done".to_string()],
        );
        ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command,
            args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: Some(ProcessResourceLimits {
                kill_on_parent_exit: true,
                ..Default::default()
            }),
            authorization_command: Some("refine test operation helper".to_string()),
            sensitive: false,
            metadata: serde_json::from_value(json!({
                "kind": "runner",
                "worker_kind": "operation-capability-test-helper",
                "operation_id": operation_id
            }))
            .unwrap(),
        }
    }

    fn wait_for_managed_pid_exit(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while managed_pid_is_alive(pid).unwrap_or(false) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
