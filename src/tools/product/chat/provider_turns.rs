use super::*;

impl FileChatService {
    pub fn resume_provider_turn(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        let capacity = self
            .try_turn_capacity(&self.load_record(session_id)?)?
            .ok_or_else(|| {
                RefineError::Conflict("automation concurrency limit reached".to_string())
            })?;
        let (record, provider_session_id) = {
            let _guard = self.acquire_session_lock(session_id)?;
            let mut record = self.load_record(session_id)?;
            if record.closed {
                return Err(RefineError::Conflict(format!(
                    "Chat session {session_id} is closed"
                )));
            }
            let Some(provider_session_id) = record.provider_session_id.clone() else {
                return Err(RefineError::InvalidInput(format!(
                    "Chat session {session_id} does not have a provider session id"
                )));
            };
            record.in_flight = true;
            record.last_turn_started_at = Some(now_timestamp());
            record.updated_at = now_timestamp();
            record.transcript_events.push(chat_event(
                "progress",
                "Resuming provider session.",
                true,
                Some(provider_session_id.clone()),
                None,
            ));
            self.write_record(&record)?;
            (record, provider_session_id)
        };

        let operation = self.register_provider_operation(&record, "resume")?;
        let provider = HostAgentProviderService {
            path_override: self.provider_path_override(),
            runtime_root: Some(self.runtime_root.join("agents")),
        };
        let provider_name = record.provider.clone();
        let result = provider.resume_detailed_with_output_and_metadata(
            &provider_name,
            &provider_session_id,
            chat_process_metadata(&record),
            |line| {
                let _ = self.append_provider_activity_progress(session_id, &line);
            },
        );
        let _guard = self.acquire_session_lock(session_id)?;
        let mut latest = self.load_record(session_id)?;
        if latest.closed {
            latest.in_flight = false;
            latest.queue_dispatching = false;
            latest.last_turn_started_at = None;
            latest.transcript_events.push(chat_event(
                "progress",
                "Managed provider process exited after cancellation.",
                true,
                latest.provider_session_id.clone(),
                Some(json!({"source": "process_supervisor"})),
            ));
            self.finish_provider_operation(
                &operation.id,
                OperationState::Cancelled,
                "Provider session resume cancelled after managed process exit",
            )?;
        } else {
            match result {
                Ok(result) => {
                    self.apply_provider_success(&mut latest, result, "Provider session resumed.");
                    self.finish_provider_operation(
                        &operation.id,
                        OperationState::Succeeded,
                        "Provider session resumed",
                    )?;
                }
                Err(error) => {
                    let detail =
                        format!("Provider session resume failed; transcript preserved: {error}");
                    self.apply_provider_failure(&mut latest, detail);
                    self.finish_provider_operation(
                        &operation.id,
                        OperationState::Failed,
                        "Provider session resume failed",
                    )?;
                }
            }
        }
        latest.updated_at = now_timestamp();
        self.write_record(&latest)?;
        drop(_guard);
        drop(capacity);
        Ok(latest)
    }

    pub fn recover_interrupted_turns(&self, detail: &str) -> RefineResult<Vec<ChatSessionRecord>> {
        let message = detail.trim();
        let registry = self.operation_registry();
        let mut recovered_session_ids = Vec::new();
        for operation in registry.recover()? {
            let Some(session_id) = chat_session_id_from_operation(&operation) else {
                continue;
            };
            if !matches!(
                operation.state,
                OperationState::Pending
                    | OperationState::Running
                    | OperationState::Cancelling
                    | OperationState::Interrupted
            ) {
                continue;
            }
            let mut record = match self.load_record(session_id) {
                Ok(record) => record,
                Err(RefineError::NotFound(_)) => {
                    if !matches!(operation.state, OperationState::Interrupted) {
                        registry.finish(&operation.id, OperationState::Interrupted)?;
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
            if record.interrupted && record.interruption_detail.as_deref() == Some(message) {
                continue;
            }
            self.mark_record_interrupted(&mut record, message);
            self.write_record(&record)?;
            if !matches!(operation.state, OperationState::Interrupted) {
                registry.finish(&operation.id, OperationState::Interrupted)?;
            }
            recovered_session_ids.push(record.id);
        }

        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut recovered = Vec::new();
        for entry in fs::read_dir(&sessions_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read chat sessions directory {}: {error}",
                sessions_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read chat session entry {}: {error}",
                    sessions_dir.display()
                ))
            })?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read_to_string(entry.path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read chat session {}: {error}",
                    entry.path().display()
                ))
            })?;
            let mut record: ChatSessionRecord = serde_json::from_str(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse chat session {}: {error}",
                    entry.path().display()
                ))
            })?;
            if !record.in_flight && !record.queue_dispatching {
                continue;
            }
            self.mark_record_interrupted(&mut record, message);
            self.write_record(&record)?;
            if !recovered_session_ids.contains(&record.id) {
                recovered_session_ids.push(record.id.clone());
                recovered.push(record);
            }
        }
        for session_id in recovered_session_ids {
            if recovered.iter().any(|record| record.id == session_id) {
                continue;
            }
            recovered.push(self.load_record(&session_id)?);
        }
        Ok(recovered)
    }

    pub(super) fn apply_provider_success(
        &self,
        record: &mut ChatSessionRecord,
        result: ProviderInvocationResult,
        progress_message: &str,
    ) {
        if let Some(provider_session_id) = result.provider_session_id {
            record.provider_session_id = Some(provider_session_id);
        }
        let artifacts = importable_artifacts_from_output(&result.output);
        if !artifacts.is_empty() {
            record.importable_artifacts.extend(artifacts.clone());
            record.transcript_events.push(chat_event(
                "system",
                &format!("Detected {} importable artifact(s).", artifacts.len()),
                true,
                record.provider_session_id.clone(),
                Some(json!({"importable_artifacts": artifacts})),
            ));
        }
        record.transcript_events.push(chat_event(
            "assistant",
            nonempty_or(&result.output, "(provider returned no output)"),
            false,
            record.provider_session_id.clone(),
            None,
        ));
        record.transcript_events.push(chat_event(
            "progress",
            progress_message,
            true,
            record.provider_session_id.clone(),
            None,
        ));
        record.in_flight = false;
        record.last_turn_started_at = None;
        record.interrupted = false;
        record.interruption_detail = None;
    }

    pub(super) fn apply_provider_failure(&self, record: &mut ChatSessionRecord, detail: String) {
        record.transcript_events.push(chat_event(
            "system",
            &detail,
            false,
            record.provider_session_id.clone(),
            None,
        ));
        record.in_flight = false;
        record.last_turn_started_at = None;
        record.interrupted = true;
        record.interruption_detail = Some(detail.clone());
    }

    pub(super) fn append_provider_activity_progress(
        &self,
        session_id: &str,
        line: &str,
    ) -> RefineResult<()> {
        let text = line.trim();
        if text.is_empty() {
            return Ok(());
        }
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let duplicate = record.transcript_events.iter().rev().take(20).any(|event| {
            event_bool(event, "progress") && event_text(event).as_deref() == Some(text)
        });
        if duplicate {
            return Ok(());
        }
        record.transcript_events.push(chat_event(
            "progress",
            text,
            true,
            record.provider_session_id.clone(),
            Some(json!({"source": "provider_output"})),
        ));
        record.updated_at = now_timestamp();
        self.write_record(&record)
    }

    pub(super) fn mark_record_interrupted(&self, record: &mut ChatSessionRecord, detail: &str) {
        record.in_flight = false;
        record.queue_dispatching = false;
        record.last_turn_started_at = None;
        record.interrupted = true;
        record.interruption_detail = Some(detail.to_string());
        record.updated_at = now_timestamp();
        record.transcript_events.push(chat_event(
            "system",
            detail,
            false,
            record.provider_session_id.clone(),
            None,
        ));
    }

    pub(super) fn operation_registry(&self) -> FileOperationRegistry {
        FileOperationRegistry::new(&self.runtime_root)
    }

    pub(super) fn try_turn_capacity(
        &self,
        _record: &ChatSessionRecord,
    ) -> RefineResult<Option<ChatCapacityPermit>> {
        Ok(Some(ChatCapacityPermit))
    }

    pub(super) fn register_provider_operation(
        &self,
        record: &ChatSessionRecord,
        operation_kind: &str,
    ) -> RefineResult<OperationHandle> {
        let registry = self.operation_registry();
        let operation = registry.register(&format!("chat:{}", record.id))?;
        let mut details = JsonObject::new();
        details.insert("session_id".to_string(), json!(record.id));
        details.insert("provider".to_string(), json!(record.provider));
        details.insert("mode".to_string(), json!(record.mode));
        details.insert("operation".to_string(), json!(operation_kind));
        registry.append_log(
            &operation.id,
            chat_operation_log("info", "Chat provider operation started", Some(details)),
        )?;
        Ok(operation)
    }

    pub(super) fn finish_provider_operation(
        &self,
        operation_id: &str,
        state: OperationState,
        message: &str,
    ) -> RefineResult<OperationHandle> {
        let registry = self.operation_registry();
        registry.append_log(operation_id, chat_operation_log("info", message, None))?;
        registry.finish(operation_id, state)
    }

    pub(super) fn session_has_active_operation(&self, session_id: &str) -> RefineResult<bool> {
        Ok(self
            .operation_registry()
            .recover()?
            .into_iter()
            .any(|operation| {
                chat_session_id_from_operation(&operation) == Some(session_id)
                    && matches!(
                        operation.state,
                        OperationState::Pending
                            | OperationState::Running
                            | OperationState::Cancelling
                    )
            }))
    }

    pub(super) fn managed_process_roots(&self) -> [PathBuf; 2] {
        [self.runtime_root.join("agents"), self.runtime_root.clone()]
    }

    pub(super) fn session_managed_processes(
        &self,
        session_id: &str,
    ) -> RefineResult<Vec<(PathBuf, String)>> {
        let mut matches = Vec::new();
        for root in self.managed_process_roots() {
            for process in FileProcessSupervisor::new(&root).list()? {
                let belongs_to_session = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                    .and_then(|details| {
                        details
                            .get("session_id")
                            .and_then(Value::as_str)
                            .map(|value| value == session_id)
                    })
                    .unwrap_or(false);
                if belongs_to_session {
                    matches.push((root.clone(), process.id));
                }
            }
        }
        Ok(matches)
    }

    pub(super) fn request_session_process_termination(
        &self,
        session_id: &str,
    ) -> RefineResult<usize> {
        let processes = self.session_managed_processes(session_id)?;
        for (root, process_id) in &processes {
            match FileProcessSupervisor::new(root).request_termination(process_id, "terminate") {
                Ok(_) | Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(processes.len())
    }
}
