use super::*;

impl FileProcessControlService {
    pub fn stop(&self, process_id: &str, signal: &str) -> RefineResult<Value> {
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
                return self.stop_managed_agent(supervisor, process, signal);
            }
            let mut stopped = supervisor.signal(process_id, signal)?;
            stopped.state = "stopped".to_string();
            return Ok(json!({
                "stopped": true,
                "process": stopped.api_json()
            }));
        }
        if let Some(session_id) = process_id.strip_prefix("chat-session-") {
            return self.stop_synthetic_chat(process_id, session_id, signal);
        }
        Err(RefineError::NotFound(format!(
            "Process {process_id} was not found"
        )))
    }

    pub fn cancel_workflow_execution(&self, execution_id: &str) -> RefineResult<Value> {
        let execution_id = execution_id.trim();
        if execution_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "workflow execution id is required".to_string(),
            ));
        }
        let _workflow_registration_lock =
            acquire_workflow_process_registration_lock(&self.runtime_root)?;
        if let Some(refine_dir) = self.refine_dir.as_deref()
            && let Some(replayed) = self.replay_cancellation_settlement(refine_dir, execution_id)?
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
            return Ok(json!({
                "cancelled": true,
                "execution_id": execution_id,
                "claim_id": claim.claim_id,
                "goal_id": claim.goal_id,
                "already_cancelled": true
            }));
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
                    || process_ownership.execution_id != execution_id
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
                execution_id: execution_id.to_string(),
                round_idx: None,
            });
        }
        if ownership.is_empty() && refine_dir.is_none() {
            ownership.push(WorkflowGoalOwnership {
                process_id: format!("workflow execution {execution_id}"),
                claim_id: claim.claim_id.clone(),
                execution_id: execution_id.to_string(),
                round_idx: None,
            });
        }

        let mut terminations = recovered
            .into_iter()
            .map(|recovered| recovered.termination)
            .collect::<Vec<_>>();
        for (supervisor, process) in managed {
            let process_ownership = ownership
                .iter()
                .find(|ownership| ownership.process_id == process.id);
            terminations.push(self.terminate_with_retained_outcome(
                &supervisor,
                &process,
                "terminate",
                Some(&claim.goal_id),
                process_ownership,
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
                ) {
                    Ok(goal) => Some(goal.goal),
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
                goal.is_some(),
                true,
            )?;
        }
        Ok(json!({
            "cancelled": true,
            "execution_id": execution_id,
            "claim_id": claim.claim_id,
            "goal_id": claim.goal_id,
            "processes": terminations,
            "goal": goal
        }))
    }

    pub(super) fn stop_managed_agent(
        &self,
        supervisor: FileProcessSupervisor,
        process: ManagedProcess,
        signal: &str,
    ) -> RefineResult<Value> {
        let process_value = process.api_json();
        let goal_id = process_value
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string);
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
        let goal = match (refine_dir.as_deref(), goal_id.as_deref()) {
            (Some(refine_dir), Some(goal_id)) => {
                let goal_fence = goal_fence.as_ref().ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Goal {goal_id} cancellation fence was lost after process exit"
                    ))
                })?;
                let ownership = goal_fence
                    .workflow
                    .as_ref()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();
                match self.settle_goal_cancellation(
                    refine_dir,
                    goal_id,
                    &goal_fence.goal,
                    &ownership,
                ) {
                    Ok(goal) => Some(goal.goal),
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
        self.complete_outcome_receipt(
            &process.id,
            goal_id.as_deref(),
            &termination,
            goal.is_some(),
            goal_fence
                .as_ref()
                .and_then(|fence| fence.workflow.as_ref())
                .is_some(),
        )?;

        let mut stopped_process = process;
        stopped_process.state = "stopped".to_string();
        let mut result = json!({
            "stopped": true,
            "process": stopped_process.api_json(),
            "termination": termination
        });
        if let Some(goal) = goal
            && let Some(object) = result.as_object_mut()
        {
            object.insert("goal".to_string(), json!(goal));
        }
        Ok(result)
    }

    pub(super) fn stop_synthetic_chat(
        &self,
        process_id: &str,
        session_id: &str,
        signal: &str,
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
        if let Some(goal_id) = goal_id.as_deref() {
            for (_, process) in &managed {
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
            terminations.push(self.terminate_with_retained_outcome(
                &supervisor,
                &process,
                signal,
                goal_id.as_deref(),
                process_ownership,
            )?);
        }
        #[cfg(test)]
        if let Some(hook) = &self.post_exit_hook {
            hook();
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
                ) {
                    Ok(goal) => Some(goal.goal),
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
        for termination in &terminations {
            self.complete_outcome_receipt(
                &termination.process_id,
                goal_id.as_deref(),
                termination,
                goal.is_some(),
                !workflow_ownership.is_empty(),
            )?;
        }
        let already_idle = terminations.is_empty();
        let mut result = json!({
            "stopped": true,
            "process": synthetic_chat_process_value(process_id, &stopped_session),
            "termination": {
                "confirmed_exit": true,
                "registry_retained_until_exit": true,
                "managed_processes": terminations,
                "already_idle": already_idle
            }
        });
        if let Some(goal) = goal
            && let Some(object) = result.as_object_mut()
        {
            object.insert("goal".to_string(), json!(goal));
        }
        Ok(result)
    }

    pub(super) fn terminate_with_retained_outcome(
        &self,
        supervisor: &FileProcessSupervisor,
        process: &ManagedProcess,
        signal: &str,
        goal_id: Option<&str>,
        ownership: Option<&WorkflowGoalOwnership>,
    ) -> RefineResult<ConfirmedProcessExit> {
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
                "recorded_at": Utc::now().to_rfc3339(),
                "termination": &confirmed,
                "confirmed_exit": true,
                "registry_cleanup_completed": false,
                "identity_cleanup_completed": false,
                "goal_cancelled": false,
                "claim_cancelled": false,
                "recovery": "the exact process exit is confirmed; retry cleanup and cancellation from the retained process-stop receipt"
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
                "recorded_at": Utc::now().to_rfc3339(),
                "termination": &cleaned,
                "confirmed_exit": true,
                "registry_cleanup_completed": true,
                "identity_cleanup_completed": true,
                "goal_cancelled": false,
                "claim_cancelled": false,
                "recovery": "cleanup is complete; retry the fenced cancellation settlement from the retained process-stop receipt"
            }),
        )
        .map_err(|error| {
            self.retain_post_exit_failure(&process.id, goal_id, json!(&cleaned), error)
        })?;
        Ok(cleaned)
    }
}
