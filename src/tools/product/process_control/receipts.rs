use super::*;

impl FileProcessControlService {
    // Receipt fields intentionally mirror the durable settlement facts instead
    // of hiding them in a positional tuple or lossy intermediate object.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn complete_outcome_receipt(
        &self,
        process_id: &str,
        goal_id: Option<&str>,
        termination: &ConfirmedProcessExit,
        goal_status: Option<&GoalStatus>,
        claim_cancelled: bool,
        worktree_retention: Option<&WorkflowWorktreeRetention>,
        requested_intent: Option<TerminationIntent>,
        termination_intent: Option<TerminationIntent>,
    ) -> RefineResult<()> {
        let retained = fs::read(
            self.runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let workflow = retained
            .as_ref()
            .and_then(|receipt| receipt.get("workflow").cloned());
        let disposition = retained
            .as_ref()
            .and_then(|receipt| receipt.get("goal_disposition").cloned());
        let retained_termination_intent = retained
            .as_ref()
            .and_then(|receipt| receipt.get("termination_intent").cloned())
            .or_else(|| {
                disposition
                    .clone()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .map(TerminationIntent::from_legacy_disposition)
                    .map(|intent| json!(intent))
            });
        let retained_requested_intent = retained
            .as_ref()
            .and_then(|receipt| receipt.get("requested_termination_intent").cloned())
            .or_else(|| retained_termination_intent.clone());
        let termination_intent = termination_intent
            .map(|intent| json!(intent))
            .or(retained_termination_intent);
        let requested_intent = requested_intent
            .map(|intent| json!(intent))
            .or(retained_requested_intent);
        let authoritative_disposition = goal_status
            .and_then(GoalStopDisposition::for_goal_status)
            .map(|disposition| json!(disposition))
            .or(disposition)
            .or_else(|| {
                termination_intent
                    .clone()
                    .and_then(|value| serde_json::from_value::<TerminationIntent>(value).ok())
                    .map(|intent| json!(intent.disposition()))
            });
        let worktree = retained
            .as_ref()
            .and_then(|receipt| receipt.get("worktree").cloned());
        let retention = worktree_retention.cloned().unwrap_or_else(|| {
            WorkflowWorktreeRetention::from_targets(
                &worktree
                    .clone()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .into_iter()
                    .collect::<Vec<_>>(),
            )
        });
        self.write_outcome_receipt(
            process_id,
            json!({
                "state": "completed",
                "process_id": process_id,
                "goal_id": goal_id,
                "workflow": workflow,
                "requested_termination_intent": requested_intent,
                "termination_intent": termination_intent,
                "goal_disposition": authoritative_disposition,
                "worktree": worktree,
                "recorded_at": Utc::now().to_rfc3339(),
                "termination": termination,
                "confirmed_exit": termination.confirmed_exit,
                "registry_cleanup_completed": termination.registry_cleanup_completed,
                "identity_cleanup_completed": termination.identity_cleanup_completed,
                "goal_cancelled": goal_status == Some(&GoalStatus::Cancelled),
                "goal_requeued": goal_status == Some(&GoalStatus::Todo),
                "goal_status": goal_status.map(GoalStatus::as_str),
                "claim_cancelled": claim_cancelled,
                "worktree_retention": &retention,
                "recovery": retention.recovery.as_deref()
            }),
        )
    }

    pub(super) fn write_outcome_receipt(
        &self,
        process_id: &str,
        receipt: Value,
    ) -> RefineResult<()> {
        write_json_receipt(
            &self
                .runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
            &receipt,
        )
    }

    pub(super) fn retain_post_exit_failure(
        &self,
        process_id: &str,
        goal_id: Option<&str>,
        termination: Value,
        cause: RefineError,
    ) -> RefineError {
        let confirmed_exit = termination_outcome_flag(&termination, "confirmed_exit");
        let registry_cleanup = termination_outcome_flag(&termination, "registry_cleanup_completed");
        let identity_cleanup = termination_outcome_flag(&termination, "identity_cleanup_completed");
        let cause_message = cause.to_string();
        let retained_context = fs::read(
            self.runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let retained_workflow = retained_context
            .as_ref()
            .and_then(|receipt| receipt.get("workflow").cloned());
        let retained_disposition = retained_context
            .as_ref()
            .and_then(|receipt| receipt.get("goal_disposition").cloned());
        let retained_intent = retained_context
            .as_ref()
            .and_then(|receipt| receipt.get("termination_intent").cloned())
            .or_else(|| {
                retained_disposition
                    .clone()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .map(TerminationIntent::from_legacy_disposition)
                    .map(|intent| json!(intent))
            });
        let retained_requested_intent = retained_context
            .as_ref()
            .and_then(|receipt| receipt.get("requested_termination_intent").cloned())
            .or_else(|| retained_intent.clone());
        let retained_worktree = retained_context
            .as_ref()
            .and_then(|receipt| receipt.get("worktree").cloned());
        let settlement = retained_workflow
            .as_ref()
            .and_then(|workflow| workflow.get("execution_id"))
            .and_then(Value::as_str)
            .and_then(|execution_id| {
                self.cancellation_settlement_journal_for_execution(execution_id)
                    .ok()
                    .flatten()
                    .map(|(_, journal)| journal)
            });
        let settlement_intent = settlement
            .as_ref()
            .and_then(|journal| journal.termination_intent)
            .map(|intent| json!(intent))
            .or(retained_intent);
        let settlement_requested_intent = settlement
            .as_ref()
            .and_then(|journal| journal.requested_termination_intent)
            .map(|intent| json!(intent))
            .or(retained_requested_intent);
        let durable_goal_status =
            self.refine_dir
                .as_deref()
                .zip(goal_id)
                .and_then(|(refine_dir, goal_id)| {
                    FileWorkItemService::new(refine_dir)
                        .show_goal_summary(goal_id)
                        .ok()
                        .map(|goal| goal.goal.status)
                });
        let authoritative_disposition = durable_goal_status
            .as_ref()
            .and_then(GoalStopDisposition::for_goal_status)
            .map(|disposition| json!(disposition))
            .or_else(|| {
                settlement
                    .as_ref()
                    .map(|journal| json!(journal.goal_disposition))
            })
            .or(retained_disposition.clone())
            .or_else(|| {
                settlement_intent
                    .clone()
                    .and_then(|value| serde_json::from_value::<TerminationIntent>(value).ok())
                    .map(|intent| json!(intent.disposition()))
            });
        let goal_cancelled = durable_goal_status
            .as_ref()
            .map(|status| *status == GoalStatus::Cancelled)
            .unwrap_or_else(|| {
                settlement
                    .as_ref()
                    .is_some_and(|journal| journal.goal_cancelled)
            });
        let goal_requeued = durable_goal_status
            .as_ref()
            .map(|status| *status == GoalStatus::Todo)
            .unwrap_or_else(|| {
                settlement
                    .as_ref()
                    .is_some_and(|journal| journal.goal_requeued)
            });
        let claim_cancelled = settlement
            .as_ref()
            .is_some_and(|journal| journal.claim_cancelled);
        let mut retained_worktrees = settlement
            .as_ref()
            .map(|journal| journal.worktrees.clone())
            .unwrap_or_default();
        if let Some(worktree) = retained_worktree
            .clone()
            .and_then(|value| serde_json::from_value(value).ok())
            && !retained_worktrees.contains(&worktree)
        {
            retained_worktrees.push(worktree);
        }
        let retention = WorkflowWorktreeRetention::from_targets(&retained_worktrees);
        let recovery = if settlement_intent.as_ref().and_then(Value::as_str)
            == Some("interactive_stop")
        {
            "inspect the retained process and settlement receipts; retry Stop for the retained process id through the shared Process capability, which preserves the original interactive requeue and retained-worktree intent"
        } else {
            "inspect the retained process and settlement receipts; retry explicit Goal or workflow cancellation through the shared Process capability"
        };
        let receipt = json!({
            "state": "partial_failure",
            "process_id": process_id,
            "goal_id": goal_id,
            "workflow": retained_workflow,
            "requested_termination_intent": settlement_requested_intent,
            "termination_intent": settlement_intent,
            "goal_disposition": authoritative_disposition,
            "worktree": retained_worktree,
            "recorded_at": Utc::now().to_rfc3339(),
            "termination": termination,
            "confirmed_exit": confirmed_exit,
            "registry_cleanup_completed": registry_cleanup,
            "identity_cleanup_completed": identity_cleanup,
            "goal_cancelled": goal_cancelled,
            "goal_requeued": goal_requeued,
            "goal_status": durable_goal_status.map(|status| status.as_str().to_string()),
            "claim_cancelled": claim_cancelled,
            "worktree_retention": &retention,
            "cause": cause_message,
            "recovery": recovery
        });
        let receipt_dir = self.runtime_root.join("process-stop-outcomes");
        let receipt_path = receipt_dir.join(format!("{process_id}.json"));
        let retained = write_json_receipt(&receipt_path, &receipt)
            .map(|()| {
                format!(
                    "retained partial-outcome evidence at {}",
                    receipt_path.display()
                )
            })
            .unwrap_or_else(|error| {
                format!(
                    "failed to retain partial-outcome evidence at {}: {error}",
                    receipt_path.display()
                )
            });
        error_with_message(
            cause,
            format!(
                "process stop reached a partial outcome{}: confirmed_exit={confirmed_exit}, registry_cleanup_completed={registry_cleanup}, identity_cleanup_completed={identity_cleanup}, goal_cancelled={goal_cancelled}, goal_requeued={goal_requeued}, claim_cancelled={claim_cancelled}, worktrees_retained={}; post-exit settlement failed: {cause_message}; {retained}; supported recovery: {recovery}",
                retention.retained,
                goal_id
                    .map(|goal_id| format!(" for Goal {goal_id}"))
                    .unwrap_or_default()
            ),
        )
    }
}
