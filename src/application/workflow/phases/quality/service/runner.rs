use super::*;

#[derive(Clone, Debug)]
pub struct QualityOperationRunner {
    pub refine_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub target_root: PathBuf,
}

impl QualityOperationRunner {
    pub fn new(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
            target_root: target_root.into(),
        }
    }

    pub fn run_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<QualityOperationResult> {
        let (operation, request) =
            self.register_goal_checks(goal_id, provider, process_metadata)?;
        self.run_registered(&operation.id, request)
    }

    pub fn start_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<OperationHandle> {
        let (operation, request) =
            self.register_goal_checks(goal_id, provider, process_metadata)?;
        let runner = self.clone();
        let operation_id = operation.id.clone();
        thread::spawn(move || {
            let _ = runner.run_registered(&operation_id, request);
        });
        Ok(operation)
    }

    /// Starts an operator-requested Quality evaluation under the same soft, observable process
    /// limits used by automated workflow work.
    pub fn start_manual_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<OperationHandle> {
        let summary = FileWorkItemService::new(&self.refine_dir).show_goal_summary(goal_id)?;
        let goal_node = summary.goal.node_id.as_deref().unwrap_or("default");
        let active_node =
            FileNodeRegistryService::with_active_root(&self.refine_dir, &self.runtime_root)
                .active_node_id()?;
        if !crate::application::fleet::nodes::node_ids_match(goal_node, &active_node) {
            return Err(RefineError::Conflict(format!(
                "Goal {} is owned by node {goal_node}, not active node {active_node}",
                summary.goal.id
            )));
        }

        let engine = WorkflowEngine::with_target_root(&self.runtime_root, &self.target_root);
        let policy = engine.policy_for_refine_dir_and_node(&self.refine_dir, &active_node)?;
        if !engine.soft_capacity_available(
            &policy,
            goal_node,
            provider,
            &self.target_root.display().to_string(),
        )? {
            return Err(RefineError::Conflict(
                "automation concurrency limit reached".to_string(),
            ));
        }

        let (operation, request) = self.register_goal_checks_for_node(
            goal_id,
            provider,
            process_metadata,
            Some(&active_node),
        )?;
        let runner = self.clone();
        let operation_id = operation.id.clone();
        thread::spawn(move || {
            let _ = runner.run_registered(&operation_id, request);
        });
        Ok(operation)
    }

    pub(crate) fn register_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<(OperationHandle, QualityCheckRequest)> {
        self.register_goal_checks_for_node(goal_id, provider, process_metadata, None)
    }

    pub(crate) fn run_registered(
        &self,
        operation_id: &str,
        mut request: QualityCheckRequest,
    ) -> RefineResult<QualityOperationResult> {
        request
            .process_metadata
            .insert("operation_id".to_string(), json!(operation_id));
        request
            .process_metadata
            .insert("kind".to_string(), json!("quality"));
        request
            .process_metadata
            .insert("goal_id".to_string(), json!(&request.owner_id));
        request
            .process_metadata
            .insert("round_idx".to_string(), json!(request.round_idx));
        request.process_metadata.insert(
            "candidate_commit".to_string(),
            json!(&request.candidate_commit),
        );
        request.process_metadata.insert(
            "runtime_root".to_string(),
            json!(self.runtime_root.display().to_string()),
        );
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let service = FileQualityService::with_runtime_root(&self.refine_dir, &self.runtime_root);
        let operation_still_owns_execution = registry.status(operation_id).is_ok_and(|operation| {
            matches!(
                operation.state,
                OperationState::Pending | OperationState::Running
            )
        });
        if operation_still_owns_execution
            && let Some(commitment) = request.identity_commitment.as_ref()
        {
            let validation = validate_quality_identity(
                &self.refine_dir,
                &self.target_root,
                &self.runtime_root,
                commitment,
                "before supervised execution",
            );
            if let Err(error) = validation {
                if super::is_quality_candidate_infrastructure(&error) {
                    self.record_candidate_infrastructure_fault(operation_id, &request, &error)?;
                } else {
                    self.record_persistence_failure(operation_id, &request, &error);
                }
                return Err(error);
            }
        }
        let execution = service.run_checks(request.clone());
        let operation_still_owns_execution = registry.status(operation_id).is_ok_and(|operation| {
            matches!(
                operation.state,
                OperationState::Pending | OperationState::Running
            )
        });
        if operation_still_owns_execution
            && let Some(commitment) = request.identity_commitment.as_ref()
            && let Err(error) = validate_quality_identity(
                &self.refine_dir,
                &self.target_root,
                &self.runtime_root,
                commitment,
                "after supervised execution",
            )
            && super::is_quality_candidate_infrastructure(&error)
        {
            self.record_candidate_infrastructure_fault(operation_id, &request, &error)?;
            return Err(error);
        }
        match execution {
            Ok(mut result) => {
                // Persist one timestamp in both the Goal proof and terminal operation so a
                // restart can recover the exact first evaluation without inventing new evidence.
                result.checked_at = Some(now_timestamp());
                let operation_message = if result.ok {
                    "Quality checks passed"
                } else {
                    result.summary.as_str()
                };
                registry.append_log(
                    operation_id,
                    quality_operation_log(
                        &request.owner_id,
                        if result.ok { "info" } else { "error" },
                        operation_message,
                        Some(json!({
                            "summary": &result.summary,
                            "candidate_commit": &result.candidate_commit,
                            "results": &result.results,
                            "diagnostics": &result.diagnostics
                        })),
                    ),
                )?;
                let current = registry.status(operation_id)?;
                match current.state {
                    OperationState::Cancelling if cancellation_requested(&current) => {
                        let operation = self.settle_cancelled(&request, operation_id)?;
                        return Ok(QualityOperationResult { operation, result });
                    }
                    OperationState::Cancelled => {
                        self.record_cancelled(&request, operation_id)?;
                        return Ok(QualityOperationResult {
                            operation: current,
                            result,
                        });
                    }
                    OperationState::Cancelling | OperationState::Interrupted => {
                        return Ok(QualityOperationResult {
                            operation: current,
                            result,
                        });
                    }
                    _ => {}
                }
                if let Err(error) = service
                    .ensure_operation_active(&request, "result settlement")
                    .and_then(|_| self.record_result(&request, &result, operation_id))
                {
                    self.record_persistence_failure(operation_id, &request, &error);
                    return Err(error);
                }
                let operation = registry.finish_with_result(
                    operation_id,
                    if result.ok {
                        OperationState::Succeeded
                    } else {
                        OperationState::Failed
                    },
                    serde_json::to_value(&result).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to encode Quality operation result: {error}"
                        ))
                    })?,
                )?;
                if matches!(operation.state, OperationState::Cancelling)
                    && cancellation_requested(&operation)
                {
                    let operation = self.settle_cancelled(&request, operation_id)?;
                    return Ok(QualityOperationResult { operation, result });
                }
                if matches!(operation.state, OperationState::Cancelled) {
                    self.record_cancelled(&request, operation_id)?;
                }
                Ok(QualityOperationResult { operation, result })
            }
            Err(error) => {
                let harness_fault = is_quality_harness_fault(&error);
                let output_contract_fault = is_quality_output_contract_fault(&error);
                let summary = quality_error_summary(&error);
                registry.append_log(
                    operation_id,
                    quality_operation_log(
                        &request.owner_id,
                        "error",
                        &summary,
                        Some(json!({
                            "error": error.to_string(),
                            "error_kind": if harness_fault {
                                "harness_fault"
                            } else if output_contract_fault {
                                "output_contract_fault"
                            } else {
                                "evaluation_error"
                            }
                        })),
                    ),
                )?;
                let current = registry.status(operation_id)?;
                match current.state {
                    OperationState::Cancelling if cancellation_requested(&current) => {
                        self.settle_cancelled(&request, operation_id)?;
                        return Err(error);
                    }
                    OperationState::Cancelled => {
                        self.record_cancelled(&request, operation_id)?;
                        return Err(error);
                    }
                    OperationState::Cancelling | OperationState::Interrupted => {
                        return Err(error);
                    }
                    _ => {}
                }
                if let Err(persistence_error) = self.record_error(&request, &error, operation_id) {
                    self.record_persistence_failure(operation_id, &request, &persistence_error);
                    // Preserve provider and authentication failures verbatim while leaving the
                    // operation nonterminal for restart recovery.
                    return Err(error);
                }
                registry.fail_with_error(
                    operation_id,
                    json!({
                        "code": if harness_fault {
                            "quality_command_harness_fault"
                        } else if output_contract_fault {
                            "quality_output_contract_repair_exhausted"
                        } else {
                            "quality_evaluation_failed"
                        },
                        "message": error.to_string()
                    }),
                )?;
                Err(error)
            }
        }
    }

    fn record_candidate_infrastructure_fault(
        &self,
        operation_id: &str,
        request: &QualityCheckRequest,
        error: &RefineError,
    ) -> RefineResult<()> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        registry.append_log(
            operation_id,
            quality_operation_log(
                &request.owner_id,
                "error",
                &error.to_string(),
                Some(json!({
                    "error": error.to_string(),
                    "error_kind": "candidate_infrastructure",
                    "evaluation_scope": request.evaluation_scope,
                    "identity_commitment": request.identity_commitment
                })),
            ),
        )?;
        registry.fail_with_error(
            operation_id,
            json!({
                "code": "quality_candidate_infrastructure_fault",
                "message": error.to_string(),
                "identity_commitment": request.identity_commitment
            }),
        )?;
        Ok(())
    }
}
