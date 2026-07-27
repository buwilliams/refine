use super::*;

impl FileProcessControlService {
    pub(super) fn recover_process_termination(
        &self,
        process_id: &str,
        requested_intent: TerminationIntent,
    ) -> RefineResult<Option<Value>> {
        let receipt_path = self
            .runtime_root
            .join("process-stop-outcomes")
            .join(format!("{process_id}.json"));
        let bytes = match fs::read(&receipt_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read retained process outcome {}: {error}",
                    receipt_path.display()
                )));
            }
        };
        let receipt: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse retained process outcome {}: {error}",
                receipt_path.display()
            ))
        })?;
        let retained_intent = receipt
            .get("termination_intent")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .or_else(|| {
                receipt
                    .get("goal_disposition")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .map(TerminationIntent::from_legacy_disposition)
            })
            .unwrap_or(TerminationIntent::ExplicitCancellation);
        if receipt
            .get("registry_cleanup_completed")
            .and_then(Value::as_bool)
            != Some(true)
            || receipt
                .get("identity_cleanup_completed")
                .and_then(Value::as_bool)
                != Some(true)
        {
            return Err(RefineError::Degraded(format!(
                "retained process outcome {} has incomplete process cleanup; retry recovery after inspecting its structured partial-failure evidence",
                receipt_path.display()
            )));
        }
        let Some(execution_id) = receipt
            .get("workflow")
            .and_then(|workflow| workflow.get("execution_id"))
            .and_then(Value::as_str)
        else {
            if let (Some(refine_dir), Some(goal_id)) = (
                self.refine_dir.as_deref(),
                receipt.get("goal_id").and_then(Value::as_str),
            ) {
                let goal = FileWorkItemService::new(refine_dir).show_goal_summary(goal_id)?;
                let authoritative_intent = retained_intent.with_authoritative_precedence(
                    requested_intent.authoritative_for_goal_status(&goal.goal.status),
                );
                if goal.goal.status == authoritative_intent.expected_goal_status() {
                    let result = self.workflow_termination_result(
                        json!({
                            "recovered_process_id": process_id,
                            "goal_id": goal_id,
                            "goal": goal.goal,
                            "termination": receipt.get("termination").cloned(),
                            "worktree_retention": receipt.get("worktree_retention").cloned()
                        }),
                        requested_intent,
                        authoritative_intent,
                    )?;
                    return Ok(Some(result));
                }
            }
            return Ok(None);
        };
        let mut recovered =
            self.cancel_workflow_execution_with_intent(execution_id, requested_intent)?;
        if let Some(object) = recovered.as_object_mut() {
            object.insert("recovered_process_id".to_string(), json!(process_id));
        }
        Ok(Some(recovered))
    }

    pub(super) fn settle_already_cancelled_claim(
        &self,
        claim: &crate::workflow::WorkflowClaim,
        execution_id: &str,
        intent: TerminationIntent,
    ) -> RefineResult<Value> {
        let Some(refine_dir) = self.refine_dir.as_deref() else {
            return Ok(json!({
                "cancelled": false,
                "partial": true,
                "claim_cancelled": true,
                "execution_id": execution_id,
                "claim_id": claim.claim_id,
                "goal_id": claim.goal_id,
                "termination_intent": intent,
                "cause": "workflow claim is cancelled but no target-app Goal store is available to verify durable Goal cancellation"
            }));
        };
        let work_items = FileWorkItemService::new(refine_dir);
        let current = work_items.show_goal_summary(&claim.goal_id)?;
        let authoritative_intent = intent.authoritative_for_goal_status(&current.goal.status);
        if current.goal.status == authoritative_intent.expected_goal_status() {
            return self.workflow_termination_result(
                json!({
                    "execution_id": execution_id,
                    "claim_id": claim.claim_id,
                    "goal_id": claim.goal_id,
                    "goal": current.goal,
                    "already_settled": true
                }),
                intent,
                authoritative_intent,
            );
        }
        if current.goal.status == GoalStatus::Done {
            return Err(RefineError::Conflict(format!(
                "workflow claim {} is cancelled but Goal {} is {}; {:?} did not overwrite terminal durable state",
                claim.claim_id,
                claim.goal_id,
                current.goal.status.as_str(),
                intent
            )));
        }
        let expectation = preflight_goal_state(refine_dir, &claim.goal_id)?;
        let recovered =
            self.recoverable_workflow_terminations(&claim.goal_id, &claim.claim_id, execution_id)?;
        let mut worktrees = recovered
            .iter()
            .filter_map(|recovered| recovered.worktree.clone())
            .collect::<Vec<_>>();
        if let Some((_, journal)) =
            self.cancellation_settlement_journal_for_execution(execution_id)?
        {
            for worktree in journal.worktrees {
                if !worktrees.contains(&worktree) {
                    worktrees.push(worktree);
                }
            }
        }
        let settlement = self.settle_goal_cancellation(
            refine_dir,
            &claim.goal_id,
            &expectation,
            &[],
            intent,
            &worktrees,
        )?;
        for recovered in recovered {
            self.complete_outcome_receipt(
                &recovered.ownership.process_id,
                Some(&claim.goal_id),
                &recovered.termination,
                Some(&settlement.goal.goal.status),
                true,
                Some(&settlement.worktree_retention),
                Some(settlement.requested_intent),
                Some(settlement.termination_intent),
            )?;
        }
        self.workflow_termination_result(
            json!({
                "execution_id": execution_id,
                "claim_id": claim.claim_id,
                "goal_id": claim.goal_id,
                "goal": settlement.goal.goal,
                "worktree_retention": settlement.worktree_retention,
                "settled_after_claim_cancellation": true
            }),
            intent,
            settlement.termination_intent,
        )
    }

    pub(super) fn workflow_termination_result(
        &self,
        mut result: Value,
        requested_intent: TerminationIntent,
        termination_intent: TerminationIntent,
    ) -> RefineResult<Value> {
        let goal_status = result
            .get("goal")
            .and_then(|goal| goal.get("status"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let object = result.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("workflow termination result must be an object".to_string())
        })?;
        object.insert(
            "requested_termination_intent".to_string(),
            json!(requested_intent),
        );
        object.insert("termination_intent".to_string(), json!(termination_intent));
        object.insert(
            "intent_superseded".to_string(),
            json!(requested_intent != termination_intent),
        );
        match requested_intent {
            TerminationIntent::ExplicitCancellation => {
                if goal_status.as_deref() == Some(GoalStatus::Cancelled.as_str()) {
                    object.insert("cancelled".to_string(), json!(true));
                } else {
                    object.insert("cancelled".to_string(), json!(false));
                    object.insert("partial".to_string(), json!(true));
                    object.insert(
                        "cause".to_string(),
                        json!(
                            "process, claim, or capacity settlement completed without verified durable Goal cancellation"
                        ),
                    );
                }
            }
            TerminationIntent::InteractiveStop => {
                let expected = termination_intent.expected_goal_status();
                if goal_status.is_some() && goal_status.as_deref() != Some(expected.as_str()) {
                    return Err(RefineError::Conflict(format!(
                        "interactive Stop resolved authoritative Goal status {} but observed {}",
                        expected.as_str(),
                        goal_status.as_deref().unwrap_or("unknown")
                    )));
                }
                object.insert("stopped".to_string(), json!(true));
                object.insert(
                    "goal_requeued".to_string(),
                    json!(goal_status.as_deref() == Some(GoalStatus::Todo.as_str())),
                );
                if termination_intent == TerminationIntent::ExplicitCancellation {
                    object.insert(
                        "cancelled".to_string(),
                        json!(goal_status.as_deref() == Some(GoalStatus::Cancelled.as_str())),
                    );
                }
            }
        }
        Ok(result)
    }
}
