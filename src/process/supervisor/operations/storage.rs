use super::*;

impl FileOperationRegistry {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn operations_dir(&self) -> PathBuf {
        self.runtime_root.join("operations")
    }

    pub(super) fn operation_path(&self, operation_id: &str) -> PathBuf {
        self.operations_dir().join(format!("{operation_id}.json"))
    }

    pub(super) fn log_path(&self, operation_id: &str) -> PathBuf {
        self.operations_dir()
            .join(format!("{operation_id}.logs.jsonl"))
    }

    pub(super) fn workflow_cancellation_path(&self, execution_id: &str) -> PathBuf {
        let encoded = execution_id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.operations_dir()
            .join(".workflow-cancellations")
            .join(format!("{encoded}.json"))
    }

    pub(super) fn workflow_cancellation(&self, execution_id: &str) -> RefineResult<Option<Value>> {
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

    pub(super) fn ensure_request_execution_active(&self, request: &Value) -> RefineResult<()> {
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

    pub(super) fn persist_workflow_cancellation(&self, execution_id: &str) -> RefineResult<Value> {
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

    pub(super) fn mutation_lock(&self) -> RefineResult<fs::File> {
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

    pub(super) fn write(&self, handle: &OperationHandle) -> RefineResult<()> {
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
}
