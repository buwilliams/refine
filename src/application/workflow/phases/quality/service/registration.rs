//! Registration and exact-source checkout selection for supervised Quality.
use super::*;

impl QualityOperationRunner {
    pub(super) fn register_goal_checks_for_node(
        &self,
        goal_id: &str,
        provider: &str,
        mut process_metadata: Map<String, Value>,
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
            && !crate::application::fleet::nodes::node_ids_match(node_id, expected_node_id)
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
        verify_quality_workflow_authority(
            &work_items,
            goal_id,
            node_id,
            round_idx,
            &process_metadata,
        )?;
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
            round.get("workflow_quality_timing").and_then(Value::as_str) == Some("post_build");
        // Current-Round integration can be admitted before the workflow persists
        // its reconciliation marker. All such regeneration uses the same exact
        // source checkout, independently of where the source branch is checked out.
        let integrated = round
            .get("workflow_integration")
            .is_some_and(Value::is_object);
        let (cwd, evaluated_commit, evaluation_scope, evaluation_branch) = if reconciliation
            .is_some()
            || post_build
            || integrated
        {
            let integration = round
                .get("workflow_integration")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} cannot regenerate isolated Quality without Governance integration evidence"
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
                    "Goal {goal_id} integrated candidate changed from {integrated_candidate} to {source_candidate_commit}"
                )));
            }
            let short_candidate = source_candidate_commit.chars().take(12).collect::<String>();
            let branch = format!(
                "refine/reconciliation/{goal_id}/round-{}/{short_candidate}",
                round_idx + 1
            );
            let git =
                FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root);
            let checkout_target = git.managed_worktree_path(&branch)?;
            let cwd = with_repository_git_lock(&self.target_root, || {
                verify_quality_workflow_authority(
                    &work_items,
                    goal_id,
                    node_id,
                    round_idx,
                    &process_metadata,
                )?;
                git.ensure_worktree_at_commit(&branch, &checkout_target, &source_candidate_commit)
                    .map(PathBuf::from)
            })?;
            let mut reconciliation = reconciliation.map(|value| Value::Object(value.clone())).unwrap_or_else(|| json!({"state": "detected", "candidate_commit": source_candidate_commit, "legacy_post_build": post_build}));
            reconciliation["quality_checkout"] = json!({
                "branch": branch,
                "path": cwd.display().to_string(),
                "candidate_commit": source_candidate_commit,
                "materialized_at": now_timestamp()
            });
            work_items.update_goal_round_evaluation_summary(
                goal_id,
                round_idx,
                &json!({"workflow_reconciliation": reconciliation}),
            )?;
            process_metadata.insert("quality_proof_mode".to_string(), json!("regenerated"));
            (
                cwd,
                source_candidate_commit.clone(),
                "isolated_candidate",
                branch,
            )
        } else {
            let branch = summary.goal.branch_name.as_deref().ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no candidate branch for Quality evaluation"
                ))
            })?;
            let git =
                FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root);
            let expected_path = git.managed_worktree_path(branch)?;
            let cwd = git.existing_worktree_for_branch(branch)?.ok_or_else(|| {
                RefineError::QualityCandidateInfrastructure(Box::new(
                    crate::error::QualityCandidateInfrastructureError {
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
            (
                cwd,
                source_candidate_commit.clone(),
                "isolated_candidate",
                branch.to_string(),
            )
        };
        let stall_timeout_seconds =
            FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root)
                .load()
                .ok()
                .map(|settings| {
                    settings
                        .get("agent_idle_timeout_seconds")
                        .and_then(Value::as_str)
                        .and_then(|value| value.trim().parse::<u64>().ok())
                        .filter(|value| *value > 0)
                        .unwrap_or(900)
                });
        validate_quality_identity(
            &self.refine_dir,
            &self.target_root,
            &self.runtime_root,
            &QualityIdentityCommitment::isolated(
                goal_id,
                round_idx,
                &evaluation_branch,
                &cwd,
                &evaluated_commit,
            ),
            "before operation registration",
        )?;
        if reconciliation.is_none()
            && !post_build
            && !integrated
            && process_metadata.contains_key("managed_worktree")
        {
            crate::infrastructure::git::worktrees::validate_workspace_launch(
                &process_metadata,
                Some(&cwd),
            )?;
        }
        let workspace = crate::infrastructure::git::worktrees::ManagedWorktree {
            repository: self.target_root.clone(),
            path: cwd.clone(),
            branch: evaluation_branch.clone(),
            commit: Some(evaluated_commit.clone()),
            allow_rebase: false,
            registration: None,
        }
        .pin()?;
        process_metadata.insert("managed_worktree".into(), json!(workspace));
        process_metadata.insert("target_app_id".into(), json!(self.target_root));
        let request = QualityCheckRequest {
            owner_id: goal_id.to_string(),
            round_idx,
            node_id: node_id.to_string(),
            provider: provider.to_string(),
            cwd: cwd.display().to_string(),
            stall_timeout_seconds,
            source_candidate_commit: Some(source_candidate_commit.clone()),
            evaluation_scope: evaluation_scope.to_string(),
            candidate_commit: evaluated_commit.clone(),
            identity_commitment: Some(QualityIdentityCommitment::isolated(
                goal_id,
                round_idx,
                &evaluation_branch,
                &cwd,
                &source_candidate_commit,
            )),
            process_metadata,
        };
        verify_quality_workflow_authority(
            &work_items,
            goal_id,
            node_id,
            round_idx,
            &request.process_metadata,
        )?;
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
                "managed_worktree": request.process_metadata.get("managed_worktree"),
                "managed_worktree": &request.process_metadata["managed_worktree"],
                "workflow_revision": request.process_metadata.get("workflow_revision"),
                "workflow_step_generation": request.process_metadata.get("workflow_step_generation"),
                "quality_proof_mode": request.process_metadata.get("quality_proof_mode"),
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
}

fn verify_quality_workflow_authority(
    work_items: &FileWorkItemService,
    goal_id: &str,
    node_id: &str,
    round_idx: usize,
    metadata: &Map<String, Value>,
) -> RefineResult<()> {
    let Some(workflow_revision) = metadata.get("workflow_revision").and_then(Value::as_u64) else {
        return Ok(());
    };
    let status = metadata
        .get("workflow_state")
        .and_then(Value::as_str)
        .and_then(GoalStatus::parse_wire)
        .ok_or_else(|| {
            RefineError::Degraded(
                "Quality workflow registration is missing its authority status".to_string(),
            )
        })?;
    work_items.verify_workflow_attempt(
        goal_id,
        WorkflowStepAuthority {
            round_idx,
            workflow_revision,
            generation: metadata.get("workflow_step_generation").and_then(Value::as_u64).ok_or_else(|| RefineError::Degraded("Quality is missing its authorized step occurrence; inspect retained evidence".into()))?,
        },
        status,
        node_id,
    )
}
