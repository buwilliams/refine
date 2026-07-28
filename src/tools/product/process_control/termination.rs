use super::*;

impl FileProcessControlService {
    /// Cancels a workflow execution through the complete shared lifecycle boundary.
    ///
    /// The durable operation tombstone brackets process/claim/Goal settlement so operation
    /// registration cannot race cancellation and publish success afterward.
    pub fn cancel_workflow_execution_managed(&self, execution_id: &str) -> RefineResult<Value> {
        let execution_id = execution_id.trim();
        let operations = FileOperationRegistry::new(&self.runtime_root);
        operations.cancel_workflow_execution_operations(execution_id)?;
        let result = self.cancel_workflow_execution_with_intent(
            execution_id,
            TerminationIntent::ExplicitCancellation,
        )?;
        operations.cancel_workflow_execution_operations(execution_id)?;
        Ok(result)
    }

    pub fn stop(&self, process_id: &str, signal: &str) -> RefineResult<Value> {
        self.terminate_process(process_id, signal, TerminationIntent::InteractiveStop)
    }

    pub(super) fn terminate_process(
        &self,
        process_id: &str,
        signal: &str,
        intent: TerminationIntent,
    ) -> RefineResult<Value> {
        validate_process_id(process_id)?;
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        if let Some((supervisor, process)) = self.find_managed_process(process_id)? {
            if is_agent_process(&process) {
                let metadata = process_metadata(&process);
                let _workflow_registration_lock = (metadata.get("claim_id").is_some()
                    && metadata.get("execution_id").is_some())
                .then(|| acquire_workflow_process_registration_lock(&self.runtime_root))
                .transpose()?;
                return self.stop_managed_agent(supervisor, process, signal, intent);
            }
            let mut stopped = supervisor.signal(process_id, signal)?;
            stopped.state = "stopped".to_string();
            return Ok(json!({
                "stopped": true,
                "process": stopped.api_json()
            }));
        }
        if let Some(session_id) = process_id.strip_prefix("chat-session-") {
            return self.stop_synthetic_chat(process_id, session_id, signal, intent);
        }
        if let Some(recovered) = self.recover_process_termination(process_id, intent)? {
            return Ok(recovered);
        }
        Err(RefineError::NotFound(format!(
            "Process {process_id} was not found"
        )))
    }

    pub fn cancel_workflow_execution(&self, execution_id: &str) -> RefineResult<Value> {
        self.cancel_workflow_execution_with_intent(
            execution_id,
            TerminationIntent::ExplicitCancellation,
        )
    }

    pub(super) fn cancel_workflow_execution_with_intent(
        &self,
        execution_id: &str,
        intent: TerminationIntent,
    ) -> RefineResult<Value> {
        let execution_id = execution_id.trim();
        if execution_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "workflow execution id is required".to_string(),
            ));
        }
        let _workflow_registration_lock =
            acquire_workflow_process_registration_lock(&self.runtime_root)?;
        if let Some(refine_dir) = self.refine_dir.as_deref()
            && let Some(replayed) =
                self.replay_cancellation_settlement(refine_dir, execution_id, intent)?
        {
            return Ok(replayed);
        }
        let state = WorkflowEngine::new(&self.runtime_root).load_state()?;
        let claim = state
            .claims
            .iter()
            .find(|claim| claim.execution_id.as_deref() == Some(execution_id))
            .cloned()
            .ok_or_else(|| {
                RefineError::NotFound(format!("claim for execution {execution_id} was not found"))
            })?;
        if claim.state == WorkflowClaimState::Cancelled {
            return self.settle_already_cancelled_claim(&claim, execution_id, intent);
        }
        if claim.state != WorkflowClaimState::Running {
            return Err(RefineError::Conflict(format!(
                "workflow execution {execution_id} is {}; only a running execution can be cancelled",
                workflow_claim_state_label(&claim.state)
            )));
        }

        let managed = self.managed_processes_for_execution(execution_id)?;
        let refine_dir = self.refine_dir.as_deref();
        let recovered = if refine_dir.is_some() && managed.is_empty() {
            self.recoverable_workflow_terminations(&claim.goal_id, &claim.claim_id, execution_id)?
        } else {
            Vec::new()
        };
        if refine_dir.is_some() && managed.is_empty() && recovered.is_empty() {
            return Err(RefineError::Conflict(format!(
                "running target-bound workflow execution {execution_id} has no managed-process record; an empty lookup is not confirmed process exit, so claim {} and Goal {} remain active and capacity remains reserved; retry after registration completes or recover the missing process evidence",
                claim.claim_id, claim.goal_id
            )));
        }
        let expectation = refine_dir
            .map(|refine_dir| preflight_goal_state(refine_dir, &claim.goal_id))
            .transpose()?;
        let mut ownership = recovered
            .iter()
            .map(|recovered| recovered.ownership.clone())
            .collect::<Vec<_>>();
        if let Some(refine_dir) = refine_dir {
            for (_, process) in &managed {
                let fence = preflight_goal_for_process(
                    refine_dir,
                    &self.runtime_root,
                    &claim.goal_id,
                    process,
                    WorkflowOwnershipPhase::BeforeTermination,
                )?;
                let process_ownership = fence.workflow.ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "managed process {} has no exact workflow ownership; termination was not requested",
                        process.id
                    ))
                })?;
                if process_ownership.claim_id != claim.claim_id
                    || process_ownership.execution_id.as_deref() != Some(execution_id)
                {
                    return Err(stale_workflow_ownership(
                        &claim.goal_id,
                        &process_ownership,
                        "the process does not belong to the requested workflow execution",
                        WorkflowOwnershipPhase::BeforeTermination,
                    ));
                }
                ownership.push(process_ownership);
            }
        } else {
            ownership.push(WorkflowGoalOwnership {
                process_id: format!("workflow execution {execution_id}"),
                claim_id: claim.claim_id.clone(),
                execution_id: Some(execution_id.to_string()),
                round_idx: None,
            });
        }
        if ownership.is_empty() && refine_dir.is_none() {
            ownership.push(WorkflowGoalOwnership {
                process_id: format!("workflow execution {execution_id}"),
                claim_id: claim.claim_id.clone(),
                execution_id: Some(execution_id.to_string()),
                round_idx: None,
            });
        }

        let mut recovered_worktrees = recovered
            .iter()
            .filter_map(|recovered| recovered.worktree.clone())
            .collect::<Vec<_>>();
        let mut terminations = recovered
            .into_iter()
            .map(|recovered| recovered.termination)
            .collect::<Vec<_>>();
        for (supervisor, process) in managed {
            let process_ownership = ownership
                .iter()
                .find(|ownership| ownership.process_id == process.id);
            let process_worktree = workflow_worktree(&process)?;
            if let Some(worktree) = process_worktree.clone()
                && !recovered_worktrees.contains(&worktree)
            {
                recovered_worktrees.push(worktree);
            }
            terminations.push(self.terminate_with_retained_outcome(
                &supervisor,
                &process,
                "terminate",
                Some(&claim.goal_id),
                process_ownership,
                intent,
                process_worktree.as_ref(),
            )?);
        }
        #[cfg(test)]
        if let Some(hook) = &self.post_exit_hook {
            hook();
        }

        let goal = match (refine_dir, expectation.as_ref()) {
            (Some(refine_dir), Some(expectation)) => {
                match self.settle_goal_cancellation(
                    refine_dir,
                    &claim.goal_id,
                    expectation,
                    &ownership,
                    intent,
                    &recovered_worktrees,
                ) {
                    Ok(settlement) => Some(settlement),
                    Err(error) => {
                        let mut retained_error = error;
                        for termination in &terminations {
                            retained_error = self.retain_post_exit_failure(
                                &termination.process_id,
                                Some(&claim.goal_id),
                                json!(termination),
                                retained_error,
                            );
                        }
                        return Err(retained_error);
                    }
                }
            }
            _ => {
                self.settle_claim_cancellation_only(&claim.goal_id, &ownership)?;
                None
            }
        };
        for termination in &terminations {
            self.complete_outcome_receipt(
                &termination.process_id,
                Some(&claim.goal_id),
                termination,
                goal.as_ref().map(|settlement| &settlement.goal.goal.status),
                true,
                goal.as_ref()
                    .map(|settlement| &settlement.worktree_retention),
                Some(intent),
                goal.as_ref()
                    .map(|settlement| settlement.termination_intent),
            )?;
        }
        self.workflow_termination_result(
            json!({
                "execution_id": execution_id,
                "claim_id": claim.claim_id,
                "goal_id": claim.goal_id,
                "processes": terminations,
                "goal": goal.as_ref().map(|settlement| &settlement.goal.goal),
                "worktree_retention": goal.as_ref().map(|settlement| &settlement.worktree_retention)
            }),
            intent,
            goal.as_ref()
                .map(|settlement| settlement.termination_intent)
                .unwrap_or(intent),
        )
    }

    pub(super) fn stop_managed_agent(
        &self,
        supervisor: FileProcessSupervisor,
        process: ManagedProcess,
        signal: &str,
        intent: TerminationIntent,
    ) -> RefineResult<Value> {
        let process_value = process.api_json();
        let goal_id = process_value
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let worktree = if goal_id.is_some() {
            workflow_worktree(&process)?
        } else {
            None
        };
        let chat_session_id = (process_value.get("kind").and_then(Value::as_str) == Some("chat"))
            .then(|| {
                process_value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .flatten();
        let refine_dir = if goal_id.is_some() || chat_session_id.is_some() {
            Some(self.resolve_refine_dir()?)
        } else {
            None
        };
        let goal_fence = match (refine_dir.as_deref(), goal_id.as_deref()) {
            (Some(refine_dir), Some(goal_id)) => Some(preflight_goal_for_process(
                refine_dir,
                &self.runtime_root,
                goal_id,
                &process,
                WorkflowOwnershipPhase::BeforeTermination,
            )?),
            _ => None,
        };
        if let (Some(refine_dir), Some(session_id)) =
            (refine_dir.as_deref(), chat_session_id.as_deref())
        {
            preflight_chat(refine_dir, &self.runtime_root, session_id)?;
        }

        let termination = self.terminate_with_retained_outcome(
            &supervisor,
            &process,
            signal,
            goal_id.as_deref(),
            goal_fence
                .as_ref()
                .and_then(|fence| fence.workflow.as_ref()),
            intent,
            worktree.as_ref(),
        )?;
        #[cfg(test)]
        if let Some(hook) = &self.post_exit_hook {
            hook();
        }

        if let (Some(refine_dir), Some(session_id)) =
            (refine_dir.as_deref(), chat_session_id.as_deref())
        {
            FileChatService::with_runtime_root(refine_dir, &self.runtime_root).stop(session_id)?;
        }
        let ownership = goal_fence
            .as_ref()
            .and_then(|fence| fence.workflow.as_ref())
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if let Err(error) = self.cancel_workflow_operations(&ownership) {
            return Err(self.retain_post_exit_failure(
                &process.id,
                goal_id.as_deref(),
                json!(&termination),
                error,
            ));
        }
        let goal = match (refine_dir.as_deref(), goal_id.as_deref()) {
            (Some(refine_dir), Some(goal_id)) => {
                let goal_fence = goal_fence.as_ref().ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} cancellation fence was lost after process exit"
                    ))
                })?;
                match self.settle_goal_cancellation(
                    refine_dir,
                    goal_id,
                    &goal_fence.goal,
                    &ownership,
                    intent,
                    worktree.as_slice(),
                ) {
                    Ok(settlement) => Some(settlement),
                    Err(error) => {
                        return Err(self.retain_post_exit_failure(
                            &process.id,
                            Some(goal_id),
                            json!(&termination),
                            error,
                        ));
                    }
                }
            }
            _ => None,
        };
        if let Err(error) = self.cancel_workflow_operations(&ownership) {
            return Err(self.retain_post_exit_failure(
                &process.id,
                goal_id.as_deref(),
                json!(&termination),
                error,
            ));
        }
        self.complete_outcome_receipt(
            &process.id,
            goal_id.as_deref(),
            &termination,
            goal.as_ref().map(|settlement| &settlement.goal.goal.status),
            goal_fence
                .as_ref()
                .and_then(|fence| fence.workflow.as_ref())
                .is_some(),
            goal.as_ref()
                .map(|settlement| &settlement.worktree_retention),
            Some(intent),
            goal.as_ref()
                .map(|settlement| settlement.termination_intent),
        )?;

        let mut stopped_process = process;
        stopped_process.state = "stopped".to_string();
        let authoritative_intent = goal
            .as_ref()
            .map(|settlement| settlement.termination_intent)
            .unwrap_or(intent);
        let mut result = json!({
            "stopped": true,
            "requested_termination_intent": intent,
            "termination_intent": authoritative_intent,
            "intent_superseded": intent != authoritative_intent,
            "process": stopped_process.api_json(),
            "termination": termination
        });
        if let Some(settlement) = goal
            && let Some(object) = result.as_object_mut()
        {
            object.insert("goal".to_string(), json!(&settlement.goal.goal));
            object.insert(
                "worktree_retention".to_string(),
                json!(&settlement.worktree_retention),
            );
        }
        if result.get("goal").is_some() {
            self.workflow_termination_result(result, intent, authoritative_intent)
        } else {
            Ok(result)
        }
    }

    pub(super) fn stop_synthetic_chat(
        &self,
        process_id: &str,
        session_id: &str,
        signal: &str,
        intent: TerminationIntent,
    ) -> RefineResult<Value> {
        let _workflow_registration_lock =
            acquire_workflow_process_registration_lock(&self.runtime_root)?;
        let refine_dir = self.resolve_refine_dir()?;
        let chat = FileChatService::with_runtime_root(&refine_dir, &self.runtime_root);
        let session = chat
            .list_sessions()?
            .into_iter()
            .find(|session| session.id == session_id && !session.closed)
            .ok_or_else(|| RefineError::NotFound(format!("Process {process_id} was not found")))?;
        let goal_id = match &session.attachment {
            ChatAttachment::Goal(goal_id) => Some(goal_id.clone()),
            _ => None,
        };
        let mut goal_expectation = goal_id
            .as_deref()
            .map(|goal_id| preflight_goal_state(&refine_dir, goal_id))
            .transpose()?;

        let managed = self.managed_processes_for_session(session_id)?;
        if managed.is_empty() && (session.in_flight || session.queue_dispatching) {
            return Err(stop_failure_with_goal_context(
                RefineError::Degraded(format!(
                    "chat agent process {process_id} reports active work but has no exact managed-process identity to terminate; the chat record was kept open for recovery"
                )),
                process_id,
                goal_id.as_deref(),
            ));
        }
        if managed.is_empty()
            && let Some(goal_id) = goal_id.as_deref()
        {
            ensure_goal_has_no_active_workflow_claim(&self.runtime_root, goal_id, process_id)?;
        }
        let mut workflow_ownership = Vec::new();
        let mut worktrees = Vec::new();
        if let Some(goal_id) = goal_id.as_deref() {
            for (_, process) in &managed {
                if let Some(worktree) = workflow_worktree(process)?
                    && !worktrees.contains(&worktree)
                {
                    worktrees.push(worktree);
                }
                let fence = preflight_goal_for_process(
                    &refine_dir,
                    &self.runtime_root,
                    goal_id,
                    process,
                    WorkflowOwnershipPhase::BeforeTermination,
                )?;
                if goal_expectation.is_none() {
                    goal_expectation = Some(fence.goal.clone());
                }
                if let Some(ownership) = fence.workflow {
                    workflow_ownership.push(ownership);
                }
            }
        }
        let mut terminations = Vec::new();
        for (supervisor, process) in managed {
            let process_ownership = workflow_ownership
                .iter()
                .find(|ownership| ownership.process_id == process.id);
            let process_worktree = if goal_id.is_some() {
                workflow_worktree(&process)?
            } else {
                None
            };
            terminations.push(self.terminate_with_retained_outcome(
                &supervisor,
                &process,
                signal,
                goal_id.as_deref(),
                process_ownership,
                intent,
                process_worktree.as_ref(),
            )?);
        }
        #[cfg(test)]
        if let Some(hook) = &self.post_exit_hook {
            hook();
        }
        let termination_summary = json!({
            "confirmed_exit": true,
            "registry_cleanup_completed": true,
            "identity_cleanup_completed": true,
            "managed_processes": &terminations,
            "already_idle": terminations.is_empty()
        });
        if let Err(error) = self.cancel_workflow_operations(&workflow_ownership) {
            return Err(self.retain_post_exit_failure(
                process_id,
                goal_id.as_deref(),
                termination_summary,
                error,
            ));
        }
        let stopped_session = chat.stop(session_id)?;
        let goal = match goal_id.as_deref() {
            Some(goal_id) => {
                let expectation = goal_expectation.as_ref().ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} cancellation fence was lost after process exit"
                    ))
                })?;
                match self.settle_goal_cancellation(
                    &refine_dir,
                    goal_id,
                    expectation,
                    &workflow_ownership,
                    intent,
                    &worktrees,
                ) {
                    Ok(settlement) => Some(settlement),
                    Err(error) => {
                        return Err(self.retain_post_exit_failure(
                            process_id,
                            Some(goal_id),
                            json!({
                                "confirmed_exit": true,
                                "registry_cleanup_completed": true,
                                "identity_cleanup_completed": true,
                                "managed_processes": &terminations,
                                "already_idle": terminations.is_empty()
                            }),
                            error,
                        ));
                    }
                }
            }
            None => None,
        };
        if let Err(error) = self.cancel_workflow_operations(&workflow_ownership) {
            return Err(self.retain_post_exit_failure(
                process_id,
                goal_id.as_deref(),
                json!({
                    "confirmed_exit": true,
                    "registry_cleanup_completed": true,
                    "identity_cleanup_completed": true,
                    "managed_processes": &terminations,
                    "already_idle": terminations.is_empty()
                }),
                error,
            ));
        }
        for termination in &terminations {
            self.complete_outcome_receipt(
                &termination.process_id,
                goal_id.as_deref(),
                termination,
                goal.as_ref().map(|settlement| &settlement.goal.goal.status),
                !workflow_ownership.is_empty(),
                goal.as_ref()
                    .map(|settlement| &settlement.worktree_retention),
                Some(intent),
                goal.as_ref()
                    .map(|settlement| settlement.termination_intent),
            )?;
        }
        let already_idle = terminations.is_empty();
        let authoritative_intent = goal
            .as_ref()
            .map(|settlement| settlement.termination_intent)
            .unwrap_or(intent);
        let mut result = json!({
            "stopped": true,
            "requested_termination_intent": intent,
            "termination_intent": authoritative_intent,
            "intent_superseded": intent != authoritative_intent,
            "process": synthetic_chat_process_value(process_id, &stopped_session),
            "termination": {
                "confirmed_exit": true,
                "registry_retained_until_exit": true,
                "managed_processes": terminations,
                "already_idle": already_idle
            }
        });
        if let Some(settlement) = goal
            && let Some(object) = result.as_object_mut()
        {
            object.insert("goal".to_string(), json!(&settlement.goal.goal));
            object.insert(
                "worktree_retention".to_string(),
                json!(&settlement.worktree_retention),
            );
        }
        if result.get("goal").is_some() {
            self.workflow_termination_result(result, intent, authoritative_intent)
        } else {
            Ok(result)
        }
    }

    // These values form the complete pre-termination ownership fence.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn terminate_with_retained_outcome(
        &self,
        supervisor: &FileProcessSupervisor,
        process: &ManagedProcess,
        signal: &str,
        goal_id: Option<&str>,
        ownership: Option<&WorkflowGoalOwnership>,
        intent: TerminationIntent,
        worktree: Option<&WorkflowWorktree>,
    ) -> RefineResult<ConfirmedProcessExit> {
        let disposition = intent.disposition();
        let worktree_retention = WorkflowWorktreeRetention::from_targets(
            &worktree.cloned().into_iter().collect::<Vec<_>>(),
        );
        let confirmed = supervisor
            .terminate_owned_and_confirm_exit(process, signal, self.agent_exit_timeout)
            .map_err(|error| stop_failure_with_goal_context(error, &process.id, goal_id))?;
        self.write_outcome_receipt(
            &process.id,
            json!({
                "state": "confirmed_exit_cleanup_pending",
                "process_id": process.id,
                "goal_id": goal_id,
                "workflow": ownership.map(workflow_ownership_json),
                "requested_termination_intent": intent,
                "termination_intent": intent,
                "goal_disposition": disposition,
                "worktree": worktree,
                "recorded_at": Utc::now().to_rfc3339(),
                "termination": &confirmed,
                "confirmed_exit": true,
                "registry_cleanup_completed": false,
                "identity_cleanup_completed": false,
                "goal_cancelled": false,
                "goal_requeued": false,
                "claim_cancelled": false,
                "worktree_retention": &worktree_retention,
                "recovery": "the exact process exit is confirmed and every workflow worktree and branch remains retained; retry process cleanup and stop settlement from the retained process-stop receipt"
            }),
        )
        .map_err(|error| {
            self.retain_post_exit_failure(
                &process.id,
                goal_id,
                json!(&confirmed),
                error,
            )
        })?;

        #[cfg(test)]
        let cleanup =
            supervisor.cleanup_confirmed_exit_with(process, confirmed, |stage| {
                match self.cleanup_failure {
                    Some(injected) if injected == stage => Err(RefineError::Io(format!(
                        "injected {} cleanup failure",
                        match stage {
                            ProcessCleanupStage::Registry => "registry",
                            ProcessCleanupStage::Identity => "identity",
                        }
                    ))),
                    _ => Ok(()),
                }
            });
        #[cfg(not(test))]
        let cleanup = supervisor.cleanup_confirmed_exit(process, confirmed);

        let cleaned = match cleanup {
            Ok(cleaned) => cleaned,
            Err(failure) => {
                return Err(self.retain_post_exit_failure(
                    &process.id,
                    goal_id,
                    json!(&failure.outcome),
                    failure.error,
                ));
            }
        };
        self.write_outcome_receipt(
            &process.id,
            json!({
                "state": "confirmed_exit_settlement_pending",
                "process_id": process.id,
                "goal_id": goal_id,
                "workflow": ownership.map(workflow_ownership_json),
                "requested_termination_intent": intent,
                "termination_intent": intent,
                "goal_disposition": disposition,
                "worktree": worktree,
                "recorded_at": Utc::now().to_rfc3339(),
                "termination": &cleaned,
                "confirmed_exit": true,
                "registry_cleanup_completed": true,
                "identity_cleanup_completed": true,
                "goal_cancelled": false,
                "goal_requeued": false,
                "claim_cancelled": false,
                "worktree_retention": &worktree_retention,
                "recovery": "process cleanup is complete and every workflow worktree and branch remains retained; retry the fenced stop settlement from the retained process-stop receipt"
            }),
        )
        .map_err(|error| {
            self.retain_post_exit_failure(&process.id, goal_id, json!(&cleaned), error)
        })?;
        Ok(cleaned)
    }

    fn cancel_workflow_operations(&self, ownership: &[WorkflowGoalOwnership]) -> RefineResult<()> {
        let mut execution_ids = ownership
            .iter()
            .filter_map(|ownership| ownership.execution_id.as_deref())
            .collect::<Vec<_>>();
        execution_ids.sort_unstable();
        execution_ids.dedup();
        let operations = FileOperationRegistry::new(&self.runtime_root);
        for execution_id in execution_ids {
            operations.cancel_workflow_execution_operations(execution_id)?;
        }
        Ok(())
    }
}
