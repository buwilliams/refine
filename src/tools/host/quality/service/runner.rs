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
        if goal_node != active_node {
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

    fn register_goal_checks_for_node(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
        expected_node_id: Option<&str>,
    ) -> RefineResult<(OperationHandle, QualityCheckRequest)> {
        let goal_id = goal_id.trim();
        if goal_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "goal_id is required for Quality evaluation".to_string(),
            ));
        }
        let work_items = FileWorkItemService::new(&self.refine_dir);
        let summary = work_items.show_goal_summary(goal_id)?;
        let node_id = summary.goal.node_id.as_deref().unwrap_or("default");
        if let Some(expected_node_id) = expected_node_id
            && node_id != expected_node_id
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is owned by node {node_id}, not active node {expected_node_id}"
            )));
        }
        let detail = work_items.show_goal_detail(goal_id)?;
        let source_candidate_commit = detail
            .get("candidate_commit")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no recorded candidate commit for Quality evaluation"
                ))
            })?
            .to_string();
        let round_idx = summary.goal.round_count.checked_sub(1).ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no round to record Quality evidence"
            ))
        })?;
        let round = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(round_idx))
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no round {} for Quality evaluation",
                    round_idx + 1
                ))
            })?;
        let reconciliation = round
            .get("workflow_reconciliation")
            .and_then(Value::as_object)
            .filter(|evidence| {
                matches!(
                    evidence.get("state").and_then(Value::as_str),
                    Some("detected" | "revert_blocked")
                )
            });
        let post_build =
            round.get("workflow_quality_timing").and_then(Value::as_str) == Some(POST_BUILD);
        let (cwd, evaluated_commit, evaluation_scope) = if post_build || reconciliation.is_some() {
            let integration = round
                .get("workflow_integration")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} cannot run post-build Quality without Ready Merge integration evidence"
                    ))
                })?;
            let integrated_candidate = integration
                .get("candidate_commit")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} integration evidence has no candidate commit"
                    ))
                })?;
            if integrated_candidate != source_candidate_commit {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} post-build Quality candidate changed from {integrated_candidate} to {source_candidate_commit}"
                )));
            }
            let (target_commit, evaluation_scope) = if reconciliation.is_some() {
                let git = FileGitWorktreeService::with_runtime_root(
                    &self.target_root,
                    &self.runtime_root,
                );
                let target_branch = integration
                    .get("target_branch")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "Goal {goal_id} integration evidence has no target branch"
                        ))
                    })?;
                let head = git.head_ref()?;
                if head.branch.as_deref() != Some(target_branch) {
                    return Err(RefineError::Conflict(format!(
                        "Goal {goal_id} already-merged reconciliation requires target worktree {} to be on branch {target_branch}, found {}",
                        self.target_root.display(),
                        head.branch.as_deref().unwrap_or("<detached>")
                    )));
                }
                let target_commit = head.commit.ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} target branch {target_branch} has no commit"
                    ))
                })?;
                if !git.commit_is_ancestor(integrated_candidate, &target_commit)? {
                    return Err(RefineError::Conflict(format!(
                        "Goal {goal_id} candidate {integrated_candidate} is no longer present in target branch {target_branch}"
                    )));
                }
                (target_commit, "integrated_target_reconciliation")
            } else {
                let target_commit = integration
                    .get("target_commit")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "Goal {goal_id} integration evidence has no target commit"
                        ))
                    })?
                    .to_string();
                (target_commit, "integrated_target")
            };
            (self.target_root.clone(), target_commit, evaluation_scope)
        } else {
            let branch = summary.goal.branch_name.as_deref().ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no candidate branch for Quality evaluation"
                ))
            })?;
            let git =
                FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root);
            let expected_path = git
                .git_path("refine-worktrees")?
                .join(branch.replace('/', "-"));
            let cwd = git.existing_worktree_for_branch(branch)?.ok_or_else(|| {
                RefineError::QualityCandidateInfrastructure(Box::new(
                    crate::process::supervisor::errors::QualityCandidateInfrastructureError {
                        goal_id: goal_id.to_string(),
                        phase: "before operation registration".to_string(),
                        reason: "the exact candidate worktree is not registered".to_string(),
                        expected_round_idx: round_idx,
                        observed_round_idx: Some(round_idx),
                        expected_branch: branch.to_string(),
                        observed_branch: summary.goal.branch_name.clone(),
                        expected_path: expected_path.display().to_string(),
                        observed_path: None,
                        expected_registered: true,
                        observed_registered: false,
                        expected_commit: source_candidate_commit.clone(),
                        observed_commit: None,
                    },
                ))
            })?;
            (cwd, source_candidate_commit.clone(), "isolated_candidate")
        };
        let request = QualityCheckRequest {
            owner_id: goal_id.to_string(),
            round_idx,
            node_id: node_id.to_string(),
            provider: provider.to_string(),
            cwd: cwd.display().to_string(),
            source_candidate_commit: Some(source_candidate_commit.clone()),
            evaluation_scope: evaluation_scope.to_string(),
            candidate_commit: evaluated_commit.clone(),
            identity_commitment: Some(if evaluation_scope == ISOLATED_CANDIDATE {
                QualityIdentityCommitment::isolated(
                    goal_id,
                    round_idx,
                    summary.goal.branch_name.as_deref().unwrap_or_default(),
                    &cwd,
                    &source_candidate_commit,
                )
            } else {
                let integration = round
                    .get("workflow_integration")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "Goal {goal_id} integrated-target Quality has no integration evidence"
                        ))
                    })?;
                let target_branch = integration
                    .get("target_branch")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "Goal {goal_id} integration evidence has no target branch"
                        ))
                    })?;
                QualityIdentityCommitment::integrated(
                    evaluation_scope,
                    goal_id,
                    round_idx,
                    target_branch,
                    &cwd,
                    &evaluated_commit,
                    &source_candidate_commit,
                )
            }),
            process_metadata,
        };
        if let Some(commitment) = request.identity_commitment.as_ref() {
            validate_quality_identity(
                &self.refine_dir,
                &self.target_root,
                &self.runtime_root,
                commitment,
                "before operation registration",
            )?;
        }
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let owner = format!("quality:{goal_id}:{}", request.candidate_commit);
        let operation = registry.register_exclusive_with_request(
            &owner,
            json!({
                "goal_id": goal_id,
                "round_idx": round_idx,
                "node_id": node_id,
                "provider": provider,
                "cwd": &request.cwd,
                "candidate_commit": &request.candidate_commit,
                "source_candidate_commit": source_candidate_commit,
                "evaluation_scope": evaluation_scope,
                "identity_commitment": &request.identity_commitment,
                "target_root": self.target_root.display().to_string(),
                "refine_dir": self.refine_dir.display().to_string(),
                "defer_cancellation_terminal": true
            }),
        )?;
        registry.append_log(
            &operation.id,
            quality_operation_log(
                goal_id,
                "info",
                "Quality checks started",
                Some(json!({
                    "provider": provider,
                    "cwd": request.cwd,
                    "candidate_commit": request.candidate_commit,
                    "source_candidate_commit": source_candidate_commit,
                    "evaluation_scope": evaluation_scope
                })),
            ),
        )?;
        Ok((operation, request))
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
            Ok(result) => {
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
                if let Err(error) = self.record_result(&request, &result, operation_id) {
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
                let summary = quality_error_summary(&error);
                registry.append_log(
                    operation_id,
                    quality_operation_log(
                        &request.owner_id,
                        "error",
                        &summary,
                        Some(json!({
                            "error": error.to_string(),
                            "error_kind": if harness_fault { "harness_fault" } else { "evaluation_error" }
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
