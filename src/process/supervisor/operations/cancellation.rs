use super::*;

impl FileOperationRegistry {
    /// Cancels an operation and all managed processes associated with it, then refreshes the
    /// caller's projection of runtime state. Durable cancellation is written first so a worker
    /// racing with termination cannot publish success over the user's cancellation.
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

    pub(super) fn terminate_associated_processes(&self, operation_id: &str) -> RefineResult<()> {
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

    pub(super) fn persist_cancellation_failure(
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

    pub(super) fn live_owned_processes(&self, operation_id: &str) -> RefineResult<Vec<Value>> {
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
}
