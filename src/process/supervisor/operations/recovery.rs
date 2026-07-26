use super::*;

impl FileOperationRegistry {
    pub(super) fn begin_recovery(
        &self,
        operation_id: &str,
    ) -> RefineResult<Option<OperationHandle>> {
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

    pub(super) fn terminate_recovery_processes(
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

    pub(super) fn interrupt_if_active(
        &self,
        operation_id: &str,
    ) -> RefineResult<Option<OperationHandle>> {
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

    pub(super) fn fail_recovery_if_active(
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
}
