use super::*;

impl FileSourcePromotionService {
    /// Fence and settle a source-upgrade cancellation using the same attempt
    /// authority that owns the deliberately external restart-safe helper.
    pub fn cancel_operation(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        self.cancel_operation_with(operation_id, &HostRestartSafeHandoffLauncher)
    }

    pub(crate) fn cancel_operation_with(
        &self,
        operation_id: &str,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<OperationHandle> {
        let registry = self.operation_registry();
        let handle = registry.status(operation_id)?;
        if handle.owner != "maintenance:source-upgrade" {
            return Err(RefineError::Conflict(format!(
                "operation {operation_id} is owned by {}, not source upgrade",
                handle.owner
            )));
        }
        if matches!(
            handle.state,
            OperationState::Cancelled
                | OperationState::Failed
                | OperationState::Succeeded
                | OperationState::Interrupted
        ) {
            return Ok(handle);
        }
        let prior_attempt = handle.external_attempt.clone();
        let cancelling = registry.compare_and_set(operation_id, handle.revision, |candidate| {
            candidate.state = OperationState::Cancelling;
            if let Some(request) = candidate.request.as_object_mut() {
                request.insert("cancellation_requested".to_string(), json!(true));
            }
            if let Some(attempt) = candidate.external_attempt.as_mut() {
                attempt.state = ExternalAttemptState::Cancelling;
                attempt.terminal_evidence = Some(json!({
                    "code": "source_upgrade_cancellation_requested",
                    "message": "late receipts and helper claims are fenced"
                }));
            }
            Ok(())
        })?;

        let mut operation = self
            .load_operation()?
            .filter(|operation| operation.id == operation_id)
            .unwrap_or_else(|| source_operation_from_handle(&cancelling));
        if mutation_has_started(&operation) {
            if matches!(operation.status.as_str(), "succeeded" | "failed") {
                let current = registry.status(operation_id)?;
                let terminal = registry.compare_and_set(
                    operation_id,
                    current.revision,
                    |candidate| {
                        candidate.state = if operation.status == "succeeded" {
                            OperationState::Succeeded
                        } else {
                            OperationState::Failed
                        };
                        candidate.result = json!({
                            "source_operation": &operation,
                            "cancellation": "arrived after source mutation; reconciled primary outcome"
                        });
                        candidate.error = operation.error.as_ref().map(|error| {
                            json!({
                                "code": "source_upgrade_failed_after_cancellation_request",
                                "message": error
                            })
                        });
                        if let Some(attempt) = candidate.external_attempt.as_mut() {
                            attempt.state = if operation.status == "succeeded" {
                                ExternalAttemptState::Completed
                            } else {
                                ExternalAttemptState::Failed
                            };
                            attempt.terminal_evidence = Some(candidate.result.clone());
                        }
                        Ok(())
                    },
                )?;
                operation.handoff_attempt = terminal.external_attempt.clone();
                self.write_source_projection(&operation)?;
                return Ok(terminal);
            }
            registry.record_recoverable_failure(
                operation_id,
                "source_upgrade_cancellation_waiting_for_safe_settlement",
                &RefineError::Conflict(format!(
                    "source mutation stage {} is active; cancellation is fenced and waits for identity, rollback, and restoration evidence",
                    operation.stage
                )),
            )?;
            return Err(RefineError::Conflict(
                "source-upgrade cancellation is fenced but cannot terminate a helper during source or registration mutation; recovery will reconcile the same attempt"
                    .to_string(),
            ));
        }

        if let Some(attempt) = prior_attempt.as_ref()
            && let Some(receipt) = attempt_receipt(attempt)
        {
            match launcher.observe(&receipt)? {
                HandoffObservation::Live => launcher.terminate(&receipt)?,
                HandoffObservation::Exited => {}
                HandoffObservation::IdentityMismatch(error)
                | HandoffObservation::Ambiguous(error) => {
                    registry.record_recoverable_failure(
                        operation_id,
                        "source_upgrade_cancellation_identity_ambiguous",
                        &RefineError::Conflict(error),
                    )?;
                    return Err(RefineError::Conflict(
                        "source-upgrade helper identity is ambiguous; cancellation remains fenced and recoverable"
                            .to_string(),
                    ));
                }
            }
            if matches!(launcher.observe(&receipt)?, HandoffObservation::Live) {
                registry.record_recoverable_failure(
                    operation_id,
                    "source_upgrade_cancellation_helper_still_live",
                    &RefineError::Degraded(
                        "exact restart-safe helper remains live after termination".to_string(),
                    ),
                )?;
                return Err(RefineError::Degraded(
                    "source-upgrade cancellation cannot settle while its helper remains live"
                        .to_string(),
                ));
            }
        }

        operation.primary_outcome = Some("cancelled".to_string());
        self.restore_workflow_admission(&mut operation)?;
        operation.status = "cancelled".to_string();
        operation.stage = "cancelled".to_string();
        operation.message = "Source update cancelled after helper ownership settled".to_string();
        operation.error = None;
        operation.recovery = Some(
            "The exact pre-upgrade workflow-admission intent was restored; retry starts a new fenced attempt"
                .to_string(),
        );
        operation.updated_at = now_timestamp();
        let current = registry.status(operation_id)?;
        let terminal = registry.compare_and_set(operation_id, current.revision, |candidate| {
            candidate.state = OperationState::Cancelled;
            candidate.error = None;
            candidate.result = json!({"source_operation": &operation});
            candidate.progress = json!({
                "status": operation.status,
                "stage": operation.stage,
                "message": operation.message,
                "source_operation": &operation
            });
            if let Some(attempt) = candidate.external_attempt.as_mut() {
                attempt.state = ExternalAttemptState::Cancelled;
                attempt.terminal_evidence = Some(json!({
                    "code": "source_upgrade_cancelled",
                    "helper_settled": true,
                    "workflow_admission_restored": operation.workflow_pause_restored
                }));
            }
            Ok(())
        })?;
        operation.handoff_attempt = terminal.external_attempt.clone();
        self.write_source_projection(&operation)?;
        Ok(terminal)
    }
}

fn mutation_has_started(operation: &SourcePromotionOperation) -> bool {
    operation.rollback_attempted
        || matches!(
            operation.stage.as_str(),
            "prepare_service_registration"
                | "stop_daemon"
                | "activate_source"
                | "restart_daemon"
                | "verify_daemon"
                | "post_restart_reconciliation"
                | "restore_workflow_admission"
        )
}

fn attempt_receipt(attempt: &ExternalOperationAttempt) -> Option<HandoffLaunchReceipt> {
    attempt
        .receipt
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or_else(|| {
            attempt.claimant_pid.map(|pid| HandoffLaunchReceipt {
                mechanism: attempt.mechanism.clone(),
                mechanism_identity: attempt.mechanism_identity.clone(),
                submitted_at: attempt.submitted_at.clone().unwrap_or_default(),
                executable: attempt.executable.clone(),
                argument_fingerprint: attempt.argument_fingerprint.clone(),
                pid: Some(pid),
                process_identity: attempt.claimant_identity.clone(),
            })
        })
}

fn source_operation_from_handle(handle: &OperationHandle) -> SourcePromotionOperation {
    let now = now_timestamp();
    SourcePromotionOperation {
        id: handle.id.clone(),
        status: "cancelling".to_string(),
        stage: "cancelling".to_string(),
        message: "Source update cancellation is reconciling".to_string(),
        checkout_path: handle
            .request
            .get("checkout")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        from_commit: String::new(),
        to_commit: String::new(),
        started_at: now.clone(),
        updated_at: now,
        error: None,
        rollback_attempted: false,
        rollback_succeeded: None,
        recovery: None,
        candidate_executable: None,
        service_manager: None,
        registration_updated: false,
        registration_rollback_succeeded: None,
        observed_executable: None,
        rollback_evidence: None,
        stashed_changes: None,
        agent_process_id: None,
        agent_worker_process_id: None,
        agent_provider: None,
        agent_context_path: None,
        agent_output: None,
        pre_upgrade_workflow_paused: None,
        workflow_pause_restored: None,
        primary_outcome: None,
        restoration_error: None,
        reconciliation_evidence: None,
        agent_decisions: Vec::new(),
        handoff_attempt: handle.external_attempt.clone(),
    }
}
