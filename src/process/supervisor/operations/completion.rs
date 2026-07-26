use super::*;

impl FileOperationRegistry {
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
