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
