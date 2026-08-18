use super::*;

impl QualityOperationRunner {
    pub(crate) fn record_result(
        &self,
        request: &QualityCheckRequest,
        result: &QualityCheckResult,
        operation_id: &str,
    ) -> RefineResult<()> {
        let checked_at = result.checked_at.clone().ok_or_else(|| {
            RefineError::Conflict(format!(
                "Quality operation {operation_id} has no stable result timestamp"
            ))
        })?;
        let source_candidate_commit = request
            .source_candidate_commit
            .as_deref()
            .unwrap_or(&request.candidate_commit);
        let recorded_results = result
            .results
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode durable Quality proof results: {error}"
                ))
            })?;
        let proof = QualityProof {
            schema_version: QUALITY_PROOF_SCHEMA_VERSION,
            goal_id: request.owner_id.clone(),
            round_idx: request.round_idx,
            evaluation_scope: request.evaluation_scope.clone(),
            operation_id: operation_id.to_string(),
            checked_candidate_commit: request.candidate_commit.clone(),
            source_candidate_commit: source_candidate_commit.to_string(),
            state: if result.ok { "passed" } else { "failed" }.to_string(),
            checked_at: checked_at.clone(),
            results: recorded_results,
        };
        let details = json!({
            "operation_id": operation_id,
            "candidate_commit": request.candidate_commit,
            "source_candidate_commit": request.source_candidate_commit,
            "evaluation_scope": request.evaluation_scope,
            "cwd": request.cwd,
            "results": result.results,
            "diagnostics": result.diagnostics,
            "provider_attempts": result.provider_attempts,
            "quality_proof": proof
        });
        let mut details = details;
        if let Some(mode) = request
            .process_metadata
            .get("quality_proof_mode")
            .and_then(Value::as_str)
        {
            details["quality_proof_mode"] = json!(mode);
        }
        FileWorkItemService::for_node(&self.refine_dir, &request.node_id)
            .update_goal_round_evaluation_summary(
                &request.owner_id,
                request.round_idx,
                &json!({
                    "quality_state": if result.ok { "passed" } else { "failed" },
                    "quality_message": result.summary,
                    "quality_details": details,
                    "quality_checked_at": checked_at,
                    "quality_candidate_commit": source_candidate_commit
                }),
            )?;
        self.append_goal_log(
            request,
            if result.ok { "info" } else { "error" },
            &result.summary,
            details,
        )
    }

    pub(crate) fn record_error(
        &self,
        request: &QualityCheckRequest,
        error: &RefineError,
        operation_id: &str,
    ) -> RefineResult<()> {
        let message = quality_error_summary(error);
        let harness_fault = is_quality_harness_fault(error);
        let output_contract_fault = is_quality_output_contract_fault(error);
        let details = json!({
            "operation_id": operation_id,
            "candidate_commit": request.candidate_commit,
            "error": error.to_string(),
            "error_kind": if harness_fault {
                "harness_fault"
            } else if output_contract_fault {
                "output_contract_fault"
            } else {
                "evaluation_error"
            }
        });
        FileWorkItemService::for_node(&self.refine_dir, &request.node_id)
            .update_goal_round_evaluation_summary(
                &request.owner_id,
                request.round_idx,
                &json!({
                    "quality_state": if harness_fault {
                        "harness_fault"
                    } else if output_contract_fault {
                        "output_contract_fault"
                    } else {
                        "failed"
                    },
                    "quality_message": message,
                    "quality_details": details,
                    "quality_checked_at": now_timestamp()
                }),
            )?;
        self.append_goal_log(request, "error", &message, details)
    }

    pub(crate) fn record_cancelled(
        &self,
        request: &QualityCheckRequest,
        operation_id: &str,
    ) -> RefineResult<()> {
        let message = "Quality checks cancelled.";
        let details = json!({
            "operation_id": operation_id,
            "candidate_commit": request.candidate_commit
        });
        let work_items = FileWorkItemService::for_node(&self.refine_dir, &request.node_id);
        let detail = work_items.show_goal_detail(&request.owner_id)?;
        let summary_persisted = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(request.round_idx))
            .is_some_and(|round| {
                round.get("quality_state").and_then(Value::as_str) == Some("cancelled")
                    && round
                        .get("quality_details")
                        .and_then(|details| details.get("operation_id"))
                        .and_then(Value::as_str)
                        == Some(operation_id)
            });
        if !summary_persisted {
            work_items.update_goal_round_evaluation_summary(
                &request.owner_id,
                request.round_idx,
                &json!({
                    "quality_state": "cancelled",
                    "quality_message": message,
                    "quality_details": details,
                    "quality_checked_at": now_timestamp()
                }),
            )?;
        }
        let logs = FileLogService::new(&self.refine_dir).all_round_logs(&request.owner_id)?;
        let log_persisted = logs.iter().any(|entry| {
            entry.round_idx == Some(request.round_idx)
                && entry.entry.category == "quality"
                && entry.entry.message == message
                && entry
                    .entry
                    .details
                    .as_ref()
                    .and_then(|details| details.get("operation_id"))
                    .and_then(Value::as_str)
                    == Some(operation_id)
        });
        if !log_persisted {
            self.append_goal_log(request, "warning", message, details)?;
        }
        Ok(())
    }

    pub(crate) fn settle_cancelled(
        &self,
        request: &QualityCheckRequest,
        operation_id: &str,
    ) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        // Goal evidence must not report terminal cancellation while owned work remains alive.
        // The operation launch barrier prevents new owned processes after Cancelling, so once
        // this check passes it is safe to persist evidence before the final idempotent settle.
        registry.ensure_cancellation_processes_exited(operation_id)?;
        if let Err(error) = self.record_cancelled(request, operation_id) {
            self.record_persistence_failure(operation_id, request, &error);
            return Err(error);
        }
        registry.settle_cancellation(operation_id)
    }

    /// Replays incomplete Quality cancellation settlement after generic process recovery has
    /// confirmed that no owned provider or command remains alive.
    pub fn recover_cancelled_operations(&self) -> RefineResult<Vec<OperationHandle>> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let mut recovered = Vec::new();
        for operation in registry.recover()? {
            if !recoverable_quality_cancellation(&operation) {
                continue;
            }
            if let Some(operation_refine_dir) =
                operation.request.get("refine_dir").and_then(Value::as_str)
                && Path::new(operation_refine_dir) != self.refine_dir
            {
                continue;
            }
            recovered.push(self.recover_cancelled_operation(&operation)?);
        }
        Ok(recovered)
    }

    /// Replays every deferred Quality cancellation against the target-app state identity stored
    /// on that operation. Individual app/evidence failures remain durable and visible without
    /// preventing the daemon from starting or other apps from recovering.
    pub fn recover_cancelled_operations_for_runtime(
        runtime_root: impl Into<PathBuf>,
    ) -> RefineResult<Vec<OperationHandle>> {
        let runtime_root = runtime_root.into();
        let registry = FileOperationRegistry::new(&runtime_root);
        let mut recovered = Vec::new();
        for operation in registry.recover()? {
            if !recoverable_quality_cancellation(&operation) {
                continue;
            }
            let recovery = (|| {
                let refine_dir = required_operation_request_string(&operation, "refine_dir")?;
                let target_root = required_operation_request_string(&operation, "target_root")?;
                Self::new(refine_dir, &runtime_root, target_root)
                    .recover_cancelled_operation(&operation)
            })();
            match recovery {
                Ok(operation) => recovered.push(operation),
                Err(error) => {
                    registry.record_recoverable_failure(
                        &operation.id,
                        "quality_cancellation_evidence_recovery_failed",
                        &error,
                    )?;
                }
            }
        }
        Ok(recovered)
    }

    pub(crate) fn recover_cancelled_operation(
        &self,
        operation: &OperationHandle,
    ) -> RefineResult<OperationHandle> {
        let request = QualityCheckRequest {
            stall_timeout_seconds: None,
            owner_id: required_operation_request_string(operation, "goal_id")?,
            round_idx: operation
                .request
                .get("round_idx")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "Quality operation {} has no valid round_idx for cancellation recovery",
                        operation.id
                    ))
                })?,
            node_id: match operation.request.get("node_id").and_then(Value::as_str) {
                Some(node_id) if !node_id.trim().is_empty() => node_id.to_string(),
                _ => {
                    let goal_id = required_operation_request_string(operation, "goal_id")?;
                    FileWorkItemService::new(&self.refine_dir)
                        .show_goal_summary(&goal_id)?
                        .goal
                        .node_id
                        .unwrap_or_else(|| "default".to_string())
                }
            },
            provider: required_operation_request_string(operation, "provider")?,
            cwd: required_operation_request_string(operation, "cwd")?,
            source_candidate_commit: operation
                .request
                .get("source_candidate_commit")
                .and_then(Value::as_str)
                .map(str::to_string),
            evaluation_scope: operation
                .request
                .get("evaluation_scope")
                .and_then(Value::as_str)
                .unwrap_or("isolated_candidate")
                .to_string(),
            candidate_commit: required_operation_request_string(operation, "candidate_commit")?,
            identity_commitment: operation
                .request
                .get("identity_commitment")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "Quality operation {} has invalid identity commitment: {error}",
                        operation.id
                    ))
                })?,
            process_metadata: Map::new(),
        };
        self.settle_cancelled(&request, &operation.id)
    }

    pub(crate) fn append_goal_log(
        &self,
        request: &QualityCheckRequest,
        severity: &str,
        message: &str,
        details: Value,
    ) -> RefineResult<()> {
        FileLogService::new(&self.refine_dir).append_round_log(
            &request.owner_id,
            request.round_idx,
            LogEntry {
                datetime: now_timestamp(),
                severity: severity.to_string(),
                category: "quality".to_string(),
                message: message.to_string(),
                details: details.as_object().cloned(),
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                goal_id: Some(request.owner_id.clone()),
            },
        )?;
        Ok(())
    }

    pub(crate) fn record_persistence_failure(
        &self,
        operation_id: &str,
        request: &QualityCheckRequest,
        error: &RefineError,
    ) {
        let _ = FileOperationRegistry::new(&self.runtime_root).append_log(
            operation_id,
            quality_operation_log(
                &request.owner_id,
                "error",
                "Quality evidence persistence failed; operation remains nonterminal for recovery",
                Some(json!({"error": error.to_string()})),
            ),
        );
    }
}
