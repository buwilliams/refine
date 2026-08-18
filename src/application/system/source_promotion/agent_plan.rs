use super::*;

const HANDOFF_STAGES: &[&str] = &[
    "restart_safe_handoff_preparing",
    "restart_safe_handoff",
    "build_candidate",
    "verify_idle",
    "prepare_service_registration",
    "stop_daemon",
    "activate_source",
    "restart_daemon",
    "verify_daemon",
    "post_restart_reconciliation",
    "complete",
];

impl FileSourcePromotionService {
    /// Execute one granular, deterministic capability selected by the installed
    /// upgrade Agent. Refine enforces legal transitions and host safety; the
    /// Agent owns the sequence, observation retries, and recovery choice.
    pub fn run_agent_capability(&self, operation_id: &str, action: &str) -> RefineResult<Value> {
        let executable = self.checkout_path.join("bin/refine");
        require_checkout_binary(&executable, "source-upgrade capability")?;
        self.run_agent_capability_with(
            operation_id,
            action,
            &executable,
            &HostRestartSafeHandoffLauncher,
        )
    }

    pub(crate) fn run_agent_capability_with(
        &self,
        operation_id: &str,
        action: &str,
        executable: &std::path::Path,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<Value> {
        let mut operation = self.current_operation(operation_id)?;
        let evidence = match action {
            "inspect" => {
                let registry = self.operation_registry().status(operation_id)?;
                let processes = self.correlated_processes(operation_id)?;
                json!({
                    "operation": &operation,
                    "registry_state": registry.state.as_api_status(),
                    "correlated_processes": processes,
                    "workflow_admission_paused": FileProcessSupervisor::new(&self.port_runtime_root)
                        .pause_state()?.workflow_paused
                })
            }
            "pause-admission" => {
                self.require_before_handoff(&operation, action)?;
                let paused = FileProcessSupervisor::new(&self.port_runtime_root)
                    .set_workflow_paused(true)?;
                operation.status = "running".to_string();
                operation.stage = "admission_paused".to_string();
                operation.message = "Update Agent paused new workflow claims".to_string();
                json!({"workflow_admission_paused": paused.workflow_paused})
            }
            "observe-work" => {
                self.require_before_handoff(&operation, action)?;
                let mut snapshot = self.inspect(false)?;
                self.remove_own_upgrade_work(&operation, &mut snapshot.active_work);
                operation.status = "running".to_string();
                if snapshot.active_work.is_empty() {
                    operation.stage = "work_settled".to_string();
                    operation.message = "Preserved managed work is settled".to_string();
                } else {
                    operation.stage = "observing_work".to_string();
                    operation.message = "Update Agent is waiting for preserved work".to_string();
                }
                json!({
                    "settled": snapshot.active_work.is_empty(),
                    "active_work": snapshot.active_work
                })
            }
            "refresh-source" => {
                self.require_before_handoff(&operation, action)?;
                let status = self.request_update_check(true)?;
                operation.status = "running".to_string();
                operation.stage = if status.check.in_flight {
                    "refreshing_source"
                } else {
                    "source_refreshed"
                }
                .to_string();
                operation.message = if status.check.in_flight {
                    "Supervised source refresh is running"
                } else {
                    "Supervised source refresh settled"
                }
                .to_string();
                serde_json::to_value(status).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to encode cached source status: {error}"
                    ))
                })?
            }
            "prepare-candidate" => {
                self.require_before_handoff(&operation, action)?;
                self.require_agent_decision(&operation, "pause-admission")?;
                self.require_settled_observation(&operation)?;
                self.require_agent_decision(&operation, "refresh-source")?;
                let cached = self.inspect_cached()?;
                if cached.check.in_flight {
                    return Err(RefineError::Conflict(
                        "supervised source refresh is still running; inspect and retry candidate preparation"
                            .to_string(),
                    ));
                }
                if let Some(failure) = cached.check.failure.as_deref() {
                    return Err(RefineError::Degraded(format!(
                        "source refresh did not produce candidate evidence: {failure}"
                    )));
                }
                let mut snapshot = cached.source;
                // Source identity comes from the supervised fetch. Managed-work
                // liveness is read now so the completed check worker is not
                // mistaken for a promotion blocker from its own snapshot.
                snapshot.active_work = self.active_work()?;
                self.remove_own_upgrade_work(&operation, &mut snapshot.active_work);
                validate_promotion(&snapshot)?;
                if snapshot.current_commit != operation.from_commit
                    || snapshot.available_commit != operation.to_commit
                {
                    return Err(RefineError::Conflict(
                        "source identities changed after authorization; check for updates and queue the update again"
                            .to_string(),
                    ));
                }
                operation.status = "running".to_string();
                operation.stage = "candidate_prepared".to_string();
                operation.message =
                    "Candidate evidence is valid for restart-safe preparation".to_string();
                json!({
                    "prepared": true,
                    "from_commit": snapshot.current_commit,
                    "to_commit": snapshot.available_commit,
                    "clean": snapshot.clean,
                    "fast_forward": snapshot.fast_forward
                })
            }
            "handoff-promotion" => {
                self.require_before_handoff(&operation, action)?;
                self.require_agent_decision(&operation, "prepare-candidate")?;
                let mut snapshot = self.inspect(false)?;
                self.remove_own_upgrade_work(&operation, &mut snapshot.active_work);
                validate_promotion(&snapshot)?;
                if snapshot.current_commit != operation.from_commit
                    || snapshot.available_commit != operation.to_commit
                {
                    return Err(RefineError::Conflict(
                        "source identities changed before restart-safe handoff".to_string(),
                    ));
                }
                operation.status = "running".to_string();
                operation.stage = "handoff_selected".to_string();
                operation.message =
                    "Update Agent selected restart-safe source promotion; reserving an attempt"
                        .to_string();
                operation.updated_at = now_timestamp();
                self.record_agent_decision(&mut operation, action, json!({"selected": true}))?;
                self.launch_operation(&mut operation, &snapshot, executable, launcher)?;
                return Ok(json!({
                    "operation_id": operation.id,
                    "handoff_started": true,
                    "stage": operation.stage
                }));
            }
            "verify-post-restart" => {
                if !HANDOFF_STAGES.contains(&operation.stage.as_str())
                    && operation.status != "succeeded"
                {
                    return Err(RefineError::Conflict(
                        "post-restart verification requires an active or completed promotion handoff"
                            .to_string(),
                    ));
                }
                self.verify_post_restart_reconciliation(&mut operation)?;
                json!({
                    "verified": operation.status == "succeeded",
                    "reconciliation_evidence": operation.reconciliation_evidence
                })
            }
            "restore-admission" => {
                self.restore_workflow_admission(&mut operation)?;
                json!({
                    "restored": operation.workflow_pause_restored,
                    "prior_paused": operation.pre_upgrade_workflow_paused
                })
            }
            "recover" => {
                let reconciled = self.reconcile_interrupted_agent()?.ok_or_else(|| {
                    RefineError::NotFound("upgrade operation is missing".to_string())
                })?;
                return Ok(json!({"operation": reconciled, "retryable": true}));
            }
            other => {
                return Err(RefineError::InvalidInput(format!(
                    "unsupported source-upgrade capability action {other}"
                )));
            }
        };
        self.record_agent_decision(&mut operation, action, evidence.clone())?;
        Ok(json!({
            "operation_id": operation.id,
            "action": action,
            "stage": operation.stage,
            "evidence": evidence
        }))
    }

    fn require_before_handoff(
        &self,
        operation: &SourcePromotionOperation,
        action: &str,
    ) -> RefineResult<()> {
        if HANDOFF_STAGES.contains(&operation.stage.as_str())
            || matches!(
                operation.status.as_str(),
                "succeeded" | "failed" | "interrupted"
            )
        {
            return Err(RefineError::Conflict(format!(
                "capability {action} cannot mutate operation {} from {} / {}",
                operation.id, operation.status, operation.stage
            )));
        }
        Ok(())
    }

    fn require_agent_decision(
        &self,
        operation: &SourcePromotionOperation,
        action: &str,
    ) -> RefineResult<()> {
        if operation
            .agent_decisions
            .iter()
            .any(|decision| decision.get("action").and_then(Value::as_str) == Some(action))
        {
            Ok(())
        } else {
            Err(RefineError::Conflict(format!(
                "upgrade Agent must successfully invoke {action} before this capability"
            )))
        }
    }

    fn require_settled_observation(
        &self,
        operation: &SourcePromotionOperation,
    ) -> RefineResult<()> {
        let settled = operation.agent_decisions.iter().rev().find(|decision| {
            decision.get("action").and_then(Value::as_str) == Some("observe-work")
        });
        if settled
            .and_then(|decision| decision.get("evidence"))
            .and_then(|evidence| evidence.get("settled"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            Ok(())
        } else {
            Err(RefineError::Conflict(
                "upgrade Agent must observe preserved managed work settled before preparing the candidate"
                    .to_string(),
            ))
        }
    }

    fn remove_own_upgrade_work(
        &self,
        operation: &SourcePromotionOperation,
        active_work: &mut Vec<String>,
    ) {
        active_work.retain(|item| {
            item != &format!("source promotion {} is {}", operation.id, operation.status)
        });
    }

    fn record_agent_decision(
        &self,
        operation: &mut SourcePromotionOperation,
        action: &str,
        evidence: Value,
    ) -> RefineResult<()> {
        operation.agent_decisions.push(json!({
            "action": action,
            "at": now_timestamp(),
            "evidence": evidence
        }));
        operation.updated_at = now_timestamp();
        self.save_operation(operation)
    }
}
