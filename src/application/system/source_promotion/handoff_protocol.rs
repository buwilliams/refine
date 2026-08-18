use chrono::{Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};

use super::*;

const CLAIM_DEADLINE_SECONDS: i64 = 30;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HandoffFailpoint {
    None,
    BeforeSubmission,
    AfterSubmissionBeforeReceipt,
    AfterReceiptBeforeActivation,
}

trait HandoffFailureInjection: Copy {
    fn stop_before_submission(self) -> bool;
    fn stop_after_submission(self) -> bool;
    fn stop_after_receipt(self) -> bool;
}

#[cfg(test)]
impl HandoffFailureInjection for HandoffFailpoint {
    fn stop_before_submission(self) -> bool {
        self == Self::BeforeSubmission
    }

    fn stop_after_submission(self) -> bool {
        self == Self::AfterSubmissionBeforeReceipt
    }

    fn stop_after_receipt(self) -> bool {
        self == Self::AfterReceiptBeforeActivation
    }
}

#[cfg(not(test))]
impl HandoffFailureInjection for () {
    fn stop_before_submission(self) -> bool {
        false
    }

    fn stop_after_submission(self) -> bool {
        false
    }

    fn stop_after_receipt(self) -> bool {
        false
    }
}

impl FileSourcePromotionService {
    pub(super) fn launch_operation_two_phase(
        &self,
        operation: &mut SourcePromotionOperation,
        snapshot: &SourcePromotionSnapshot,
        executable: &Path,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<()> {
        #[cfg(test)]
        return self.launch_operation_two_phase_with_failpoint(
            operation,
            snapshot,
            executable,
            launcher,
            HandoffFailpoint::None,
        );
        #[cfg(not(test))]
        self.launch_operation_two_phase_inner(operation, snapshot, executable, launcher, ())
    }

    #[cfg(test)]
    pub(crate) fn launch_operation_two_phase_with_failpoint(
        &self,
        operation: &mut SourcePromotionOperation,
        snapshot: &SourcePromotionSnapshot,
        executable: &Path,
        launcher: &dyn RestartSafeHandoffLauncher,
        failpoint: HandoffFailpoint,
    ) -> RefineResult<()> {
        self.launch_operation_two_phase_inner(operation, snapshot, executable, launcher, failpoint)
    }

    fn launch_operation_two_phase_inner<F: HandoffFailureInjection>(
        &self,
        operation: &mut SourcePromotionOperation,
        snapshot: &SourcePromotionSnapshot,
        executable: &Path,
        launcher: &dyn RestartSafeHandoffLauncher,
        failpoint: F,
    ) -> RefineResult<()> {
        let runtime_root = self.port_runtime_root.parent().ok_or_else(|| {
            RefineError::InvalidInput("port runtime root has no parent".to_string())
        })?;
        let service_manager =
            FileInstallationService::for_port(runtime_root, env!("CARGO_PKG_VERSION"), self.port)
                .installed_service_manager_for(InstalledServiceAction::Stop)?;
        let attempt_id = format!("handoff-{}", Uuid::new_v4());
        let raw_nonce = Uuid::new_v4().to_string();
        let handoff = RestartSafeHandoff {
            executable: executable.to_path_buf(),
            args: vec![
                "system".to_string(),
                "source-promote-helper".to_string(),
                "--checkout".to_string(),
                snapshot.checkout_path.clone(),
                "--port-runtime-root".to_string(),
                self.port_runtime_root.display().to_string(),
                "--port".to_string(),
                self.port.to_string(),
                "--operation-id".to_string(),
                operation.id.clone(),
                "--attempt-id".to_string(),
                attempt_id.clone(),
                "--claim-nonce".to_string(),
                raw_nonce.clone(),
            ],
            cwd: self.checkout_path.clone(),
            label: format!("{}-{attempt_id}", operation.id),
        };
        let now = Utc::now();
        let attempt = ExternalOperationAttempt {
            attempt_id: attempt_id.clone(),
            state: ExternalAttemptState::Reserved,
            nonce_verifier: nonce_verifier(&raw_nonce),
            mechanism: handoff_mechanism(service_manager.as_deref())?.to_string(),
            mechanism_identity: handoff_mechanism_identity(&handoff, service_manager.as_deref())?,
            executable: executable.display().to_string(),
            argument_fingerprint: handoff_argument_fingerprint(&handoff),
            reserved_at: now.to_rfc3339(),
            claim_deadline_at: (now + ChronoDuration::seconds(CLAIM_DEADLINE_SECONDS)).to_rfc3339(),
            submitted_at: None,
            receipt: None,
            claimed_at: None,
            claimant_pid: None,
            claimant_identity: None,
            terminal_evidence: None,
        };
        operation.status = "running".to_string();
        operation.stage = "restart_safe_handoff_preparing".to_string();
        operation.message =
            "Restart-safe handoff reserved; helper liveness is not yet proven".to_string();
        operation.handoff_attempt = Some(attempt.clone());
        operation.updated_at = now_timestamp();
        let registry = self.operation_registry();
        let mut handle = registry.status(&operation.id).or_else(|_| {
            registry.register_with_id(
                &operation.id,
                "maintenance:source-upgrade",
                json!({
                    "kind": "source_upgrade",
                    "restart_recovery": "capability",
                    "defer_cancellation_terminal": true
                }),
            )
        })?;
        if handle
            .external_attempt
            .as_ref()
            .is_some_and(|current| !current.state.terminal())
        {
            return Err(RefineError::Conflict(format!(
                "source-upgrade operation {} already owns handoff attempt {}",
                operation.id,
                handle.external_attempt.as_ref().unwrap().attempt_id
            )));
        }
        handle = registry.compare_and_set(&operation.id, handle.revision, |candidate| {
            candidate.external_attempt = Some(attempt.clone());
            candidate.progress = source_progress(operation, Some(&attempt));
            Ok(())
        })?;
        operation.handoff_attempt = handle.external_attempt.clone();
        operation.updated_at = now_timestamp();
        self.write_source_projection(operation)?;

        if failpoint.stop_before_submission() {
            return Err(RefineError::Degraded(
                "injected crash before restart-safe helper submission".to_string(),
            ));
        }

        let receipt = launcher.submit(&handoff, service_manager.as_deref())?;
        if failpoint.stop_after_submission() {
            return Err(RefineError::Degraded(
                "injected crash after helper submission before receipt persistence".to_string(),
            ));
        }

        handle = registry.status(&operation.id)?;
        handle = registry.compare_and_set(&operation.id, handle.revision, |candidate| {
            let current = matching_attempt_mut(candidate, &attempt_id)?;
            if current.state.terminal() {
                return Err(RefineError::Conflict(
                    "handoff attempt became terminal before its receipt was recorded".to_string(),
                ));
            }
            current.submitted_at = Some(receipt.submitted_at.clone());
            current.receipt = Some(serde_json::to_value(&receipt).map_err(|error| {
                RefineError::Serialization(format!("failed to encode handoff receipt: {error}"))
            })?);
            if !matches!(current.state, ExternalAttemptState::Claimed) {
                current.state = ExternalAttemptState::Received;
            }
            Ok(())
        })?;
        operation.handoff_attempt = handle.external_attempt.clone();
        self.write_source_projection(operation)?;

        if failpoint.stop_after_receipt() {
            return Err(RefineError::Degraded(
                "injected crash after receipt persistence before handoff activation".to_string(),
            ));
        }

        handle = registry.compare_and_set(&operation.id, handle.revision, |candidate| {
            let current = matching_attempt_mut(candidate, &attempt_id)?;
            if !matches!(
                current.state,
                ExternalAttemptState::Received | ExternalAttemptState::Claimed
            ) {
                return Err(RefineError::Conflict(format!(
                    "handoff attempt cannot activate from {:?}",
                    current.state
                )));
            }
            current.state = ExternalAttemptState::Active;
            Ok(())
        })?;
        operation.handoff_attempt = handle.external_attempt.clone();
        operation.stage = "restart_safe_handoff".to_string();
        operation.message =
            "Restart-safe helper receipt accepted and handoff activated".to_string();
        operation.updated_at = now_timestamp();
        self.save_operation(operation)
    }

    pub(crate) fn claim_handoff(
        &self,
        operation_id: &str,
        attempt_id: &str,
        raw_nonce: &str,
    ) -> RefineResult<ExternalOperationAttempt> {
        let registry = self.operation_registry();
        let handle = registry.status(operation_id)?;
        let claimed = registry.compare_and_set(operation_id, handle.revision, |candidate| {
            if !matches!(
                candidate.state,
                OperationState::Running | OperationState::Pending
            ) {
                return Err(RefineError::Conflict(format!(
                    "operation {operation_id} no longer accepts helper claims"
                )));
            }
            let attempt = matching_attempt_mut(candidate, attempt_id)?;
            if attempt.state.terminal()
                || matches!(attempt.state, ExternalAttemptState::Cancelling)
                || attempt.nonce_verifier != nonce_verifier(raw_nonce)
            {
                return Err(RefineError::Conflict(
                    "stale, cancelled, or mismatched restart-safe helper claim rejected"
                        .to_string(),
                ));
            }
            if attempt.claimed_at.is_some() {
                return Err(RefineError::Conflict(
                    "restart-safe handoff attempt already has an authoritative claimant"
                        .to_string(),
                ));
            }
            attempt.state = ExternalAttemptState::Claimed;
            attempt.claimed_at = Some(now_timestamp());
            attempt.claimant_pid = Some(std::process::id());
            attempt.claimant_identity = helper_process_identity(std::process::id());
            Ok(())
        })?;
        claimed
            .external_attempt
            .ok_or_else(|| RefineError::NotFound("claimed handoff attempt disappeared".to_string()))
    }

    pub(crate) fn reconcile_handoff_attempt(
        &self,
        operation: &mut SourcePromotionOperation,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<bool> {
        let registry = self.operation_registry();
        let handle = match registry.status(&operation.id) {
            Ok(handle) => handle,
            Err(RefineError::NotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        let Some(attempt) = handle.external_attempt.clone() else {
            if operation.stage == "restart_safe_handoff" {
                self.interrupt_handoff_without_liveness(
                    operation,
                    "legacy restart-safe stage has no correlated attempt, receipt, or claim",
                )?;
                return Ok(true);
            }
            return Ok(false);
        };
        operation.handoff_attempt = Some(attempt.clone());
        if attempt.state.terminal() {
            self.write_source_projection(operation)?;
            return Ok(true);
        }
        let receipt = attempt
            .receipt
            .as_ref()
            .and_then(|value| serde_json::from_value::<HandoffLaunchReceipt>(value.clone()).ok())
            .unwrap_or_else(|| synthetic_receipt(&attempt));
        match launcher.observe(&receipt)? {
            HandoffObservation::Live
                if attempt.claimed_at.is_some()
                    || attempt.receipt.is_some()
                    || attempt.mechanism != "detached" =>
            {
                let updated =
                    registry.compare_and_set(&operation.id, handle.revision, |candidate| {
                        let current = matching_attempt_mut(candidate, &attempt.attempt_id)?;
                        if !current.state.terminal() {
                            current.state = ExternalAttemptState::Active;
                        }
                        Ok(())
                    })?;
                operation.handoff_attempt = updated.external_attempt;
                operation.stage = "restart_safe_handoff".to_string();
                operation.message =
                    "Adopted the exact live restart-safe helper after daemon restart".to_string();
                operation.updated_at = now_timestamp();
                self.save_operation(operation)?;
                Ok(true)
            }
            HandoffObservation::Live => Ok(true),
            observation if claim_deadline_elapsed(&attempt) => {
                let evidence = match observation {
                    HandoffObservation::Exited => "no exact helper is live".to_string(),
                    HandoffObservation::IdentityMismatch(error)
                    | HandoffObservation::Ambiguous(error) => error,
                    HandoffObservation::Live => unreachable!(),
                };
                self.interrupt_handoff_without_liveness(operation, &evidence)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn interrupt_handoff_without_liveness(
        &self,
        operation: &mut SourcePromotionOperation,
        evidence: &str,
    ) -> RefineResult<()> {
        let registry = self.operation_registry();
        if let Ok(handle) = registry.status(&operation.id) {
            let _ = registry.compare_and_set(&operation.id, handle.revision, |candidate| {
                if let Some(attempt) = candidate.external_attempt.as_mut() {
                    attempt.state = ExternalAttemptState::Interrupted;
                    attempt.terminal_evidence = Some(json!({
                        "code": "source_upgrade_handoff_interrupted",
                        "message": evidence,
                        "retryable": true
                    }));
                }
                candidate.state = OperationState::Interrupted;
                candidate.error = Some(json!({
                    "code": "source_upgrade_handoff_interrupted",
                    "message": evidence,
                    "retryable": true
                }));
                Ok(())
            });
        }
        operation.status = "interrupted".to_string();
        operation.stage = "restart_safe_handoff_interrupted".to_string();
        operation.message = "Restart-safe helper ownership could not be proven".to_string();
        operation.error = Some(evidence.to_string());
        operation.recovery = Some(
            "The source checkout was left inspectable; retry creates a newly fenced attempt only after this one is terminal"
                .to_string(),
        );
        operation.updated_at = now_timestamp();
        self.write_source_projection(operation)
    }
}

fn matching_attempt_mut<'a>(
    operation: &'a mut OperationHandle,
    attempt_id: &str,
) -> RefineResult<&'a mut ExternalOperationAttempt> {
    operation
        .external_attempt
        .as_mut()
        .filter(|attempt| attempt.attempt_id == attempt_id)
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "operation {} no longer owns handoff attempt {attempt_id}",
                operation.id
            ))
        })
}

fn nonce_verifier(raw_nonce: &str) -> String {
    format!("{:x}", Sha256::digest(raw_nonce.as_bytes()))
}

fn source_progress(
    operation: &SourcePromotionOperation,
    attempt: Option<&ExternalOperationAttempt>,
) -> Value {
    json!({
        "status": operation.status,
        "stage": operation.stage,
        "message": operation.message,
        "agent_process_id": operation.agent_process_id,
        "source_operation": operation,
        "handoff_attempt": attempt
    })
}

fn synthetic_receipt(attempt: &ExternalOperationAttempt) -> HandoffLaunchReceipt {
    HandoffLaunchReceipt {
        mechanism: attempt.mechanism.clone(),
        mechanism_identity: attempt.mechanism_identity.clone(),
        submitted_at: attempt.submitted_at.clone().unwrap_or_default(),
        executable: attempt.executable.clone(),
        argument_fingerprint: attempt.argument_fingerprint.clone(),
        pid: attempt.claimant_pid,
        process_identity: attempt.claimant_identity.clone(),
    }
}

fn claim_deadline_elapsed(attempt: &ExternalOperationAttempt) -> bool {
    chrono::DateTime::parse_from_rfc3339(&attempt.claim_deadline_at)
        .map(|deadline| Utc::now() >= deadline.with_timezone(&Utc))
        .unwrap_or(true)
}

#[cfg(target_os = "linux")]
fn helper_process_identity(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    Some(format!(
        "linux-proc-start:{}",
        after_name.split_whitespace().nth(19)?
    ))
}

#[cfg(not(target_os = "linux"))]
fn helper_process_identity(_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
#[path = "handoff_protocol_tests.rs"]
mod tests;
