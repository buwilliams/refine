use super::*;

impl FileSourcePromotionService {
    pub(crate) fn verify_post_restart_reconciliation(
        &self,
        operation: &mut SourcePromotionOperation,
    ) -> RefineResult<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut last = json!({});
        loop {
            let daemon_status = fs::read(self.port_runtime_root.join("daemon-status.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            let runner_state = daemon_status
                .as_ref()
                .and_then(|status| status.get("worker_state"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let runner_available = !matches!(
                runner_state.as_str(),
                "unknown" | "starting" | "stopped" | "failed"
            );
            let target_state_path = self.port_runtime_root.join("target-app-state.json");
            let (target_app_lifecycle, target_app_available) = match fs::read(&target_state_path) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => {
                        let state = value
                            .get("state")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string();
                        let available = value.get("ok").and_then(Value::as_bool).unwrap_or(false)
                            && !matches!(state.as_str(), "failed" | "unavailable");
                        (state, available)
                    }
                    Err(_) => ("invalid".to_string(), false),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ("detached".to_string(), true)
                }
                Err(_) => ("unavailable".to_string(), false),
            };
            let event_stream_available = source_event_stream_reachable(self.port);
            last = json!({
                "daemon_identity_verified": operation.observed_executable.is_some(),
                "runner_available": runner_available,
                "runner_state": runner_state,
                "target_app_lifecycle": target_app_lifecycle,
                "target_app_available": target_app_available,
                "event_stream_available": event_stream_available,
            });
            operation.reconciliation_evidence = Some(last.clone());
            operation.updated_at = now_timestamp();
            self.save_operation(operation)?;
            if runner_available && target_app_available && event_stream_available {
                operation.status = "succeeded".to_string();
                operation.stage = "complete".to_string();
                operation.message = "Latest source promoted and Refine is healthy".to_string();
                operation.updated_at = now_timestamp();
                self.save_operation(operation)?;
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                let error = RefineError::Degraded(format!(
                    "post-restart reconciliation did not settle: {last}"
                ));
                operation.status = "failed".to_string();
                operation.stage = "post_restart_reconciliation".to_string();
                operation.message =
                    "Refine restarted but required runtime recovery is incomplete".to_string();
                operation.error = Some(error.to_string());
                operation.recovery = Some(
                    "The promoted daemon identity is retained. Restore the runner, attached target-app lifecycle, or browser event stream named in reconciliation_evidence, then retry reconciliation"
                        .to_string(),
                );
                operation.updated_at = now_timestamp();
                self.save_operation(operation)?;
                return Err(error);
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn load_operation(&self) -> RefineResult<Option<SourcePromotionOperation>> {
        let projected = match fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice::<SourcePromotionOperation>(&bytes)
                .map(Some)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to parse source-promotion state {}: {error}",
                        self.state_path().display()
                    ))
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to read source-promotion state {}: {error}",
                self.state_path().display()
            ))),
        }?;
        let registry = self.operation_registry();
        let authoritative = match projected.as_ref() {
            Some(operation) => registry.status(&operation.id).ok(),
            None => registry.recover()?.into_iter().rev().find(|operation| {
                operation.owner == "maintenance:source-upgrade"
                    && operation.progress.get("source_operation").is_some()
            }),
        };
        if let Some(handle) = authoritative
            && let Some(value) = handle.progress.get("source_operation")
        {
            let mut operation = serde_json::from_value::<SourcePromotionOperation>(value.clone())
                .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse authoritative source operation {}: {error}",
                    handle.id
                ))
            })?;
            operation.handoff_attempt = handle.external_attempt;
            return Ok(Some(operation));
        }
        Ok(projected)
    }

    pub(crate) fn save_operation(&self, operation: &SourcePromotionOperation) -> RefineResult<()> {
        let registry = self.operation_registry();
        let mut projection = operation.clone();
        if let Ok(mut handle) = registry.status(&operation.id) {
            if let Some(attempt) = handle.external_attempt.as_ref()
                && !attempt.state.terminal()
                && matches!(
                    operation.status.as_str(),
                    "succeeded" | "failed" | "interrupted" | "cancelled"
                )
            {
                handle = registry.compare_and_set(&operation.id, handle.revision, |candidate| {
                    if let Some(attempt) = candidate.external_attempt.as_mut()
                        && !attempt.state.terminal()
                    {
                        attempt.state = match operation.status.as_str() {
                            "succeeded" => ExternalAttemptState::Completed,
                            "cancelled" => ExternalAttemptState::Cancelled,
                            "interrupted" => ExternalAttemptState::Interrupted,
                            _ => ExternalAttemptState::Failed,
                        };
                        attempt.terminal_evidence = Some(json!({
                            "status": operation.status,
                            "stage": operation.stage,
                            "message": operation.message,
                            "error": operation.error
                        }));
                    }
                    Ok(())
                })?;
            }
            projection.handoff_attempt = handle.external_attempt;
            registry.update_progress(
                &operation.id,
                json!({
                    "status": projection.status,
                    "stage": projection.stage,
                    "message": projection.message,
                    "agent_process_id": projection.agent_process_id,
                    "source_operation": &projection,
                }),
            )?;
        }
        self.write_source_projection(&projection)?;
        self.settle_registry_from_source_operation(&projection)
    }

    pub(crate) fn write_source_projection(
        &self,
        operation: &SourcePromotionOperation,
    ) -> RefineResult<()> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create port runtime root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(operation).map_err(|error| {
            RefineError::Serialization(format!("failed to encode source-promotion state: {error}"))
        })?;
        let pending = self.state_path().with_extension("json.pending");
        fs::write(&pending, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write source-promotion state {}: {error}",
                pending.display()
            ))
        })?;
        fs::rename(&pending, self.state_path()).map_err(|error| {
            RefineError::Io(format!(
                "failed to publish source-promotion state {}: {error}",
                self.state_path().display()
            ))
        })?;
        Ok(())
    }

    pub(crate) fn active_work(&self) -> RefineResult<Vec<String>> {
        let mut active = Vec::new();
        let supervisor = FileProcessSupervisor::new(&self.port_runtime_root);
        let pause_state = supervisor.pause_state()?;
        if !pause_state.workflow_paused {
            active.push("workflow automation is not paused".to_string());
        }
        for process in supervisor.list()? {
            if process.state == "running"
                && !matches!(
                    process.owner,
                    ProcessOwner::Daemon | ProcessOwner::Runner | ProcessOwner::TargetApp
                )
            {
                let own_upgrade_process = process.details.as_deref().is_some_and(|details| {
                    self.load_operation()
                        .ok()
                        .flatten()
                        .is_some_and(|operation| {
                            details.contains(&operation.id)
                                && (details.contains("source_upgrade_agent")
                                    || details.contains("source_upgrade_agent_launcher"))
                                && matches!(operation.status.as_str(), "queued" | "running")
                        })
                });
                if own_upgrade_process {
                    continue;
                }
                active.push(format!(
                    "running {} process {}",
                    process.owner.as_kind(),
                    process.id
                ));
            }
        }
        if let Some(operation) = self.load_operation()?
            && matches!(operation.status.as_str(), "queued" | "running")
        {
            active.push(format!(
                "source promotion {} is {}",
                operation.id, operation.status
            ));
        }
        active.sort();
        active.dedup();
        Ok(active)
    }
}

fn source_event_stream_reachable(port: u16) -> bool {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let Ok(mut stream) = TcpStream::connect_timeout(&address.into(), Duration::from_secs(1)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream
        .write_all(
            format!(
                "GET /api/sse HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .is_err()
    {
        return false;
    }
    let mut buffer = [0_u8; 1024];
    let Ok(count) = stream.read(&mut buffer) else {
        return false;
    };
    let response = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
    response.contains(" 200 ") && response.contains("text/event-stream")
}
