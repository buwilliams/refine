use super::*;

impl FileProcessControlService {
    pub(super) fn complete_outcome_receipt(
        &self,
        process_id: &str,
        goal_id: Option<&str>,
        termination: &ConfirmedProcessExit,
        goal_cancelled: bool,
        claim_cancelled: bool,
    ) -> RefineResult<()> {
        let workflow = fs::read(
            self.runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|receipt| receipt.get("workflow").cloned());
        self.write_outcome_receipt(
            process_id,
            json!({
                "state": "completed",
                "process_id": process_id,
                "goal_id": goal_id,
                "workflow": workflow,
                "recorded_at": Utc::now().to_rfc3339(),
                "termination": termination,
                "confirmed_exit": termination.confirmed_exit,
                "registry_cleanup_completed": termination.registry_cleanup_completed,
                "identity_cleanup_completed": termination.identity_cleanup_completed,
                "goal_cancelled": goal_cancelled,
                "claim_cancelled": claim_cancelled
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
        let recovery = "inspect the retained receipt and current Goal round and workflow claims; if cancellation is still intended, request it through the current Goal owner";
        let retained_workflow = fs::read(
            self.runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|receipt| receipt.get("workflow").cloned());
        let receipt = json!({
            "state": "partial_failure",
            "process_id": process_id,
            "goal_id": goal_id,
            "workflow": retained_workflow,
            "recorded_at": Utc::now().to_rfc3339(),
            "termination": termination,
            "confirmed_exit": confirmed_exit,
            "registry_cleanup_completed": registry_cleanup,
            "identity_cleanup_completed": identity_cleanup,
            "goal_cancelled": false,
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
                "process stop reached a partial outcome{}: confirmed_exit={confirmed_exit}, registry_cleanup_completed={registry_cleanup}, identity_cleanup_completed={identity_cleanup}, goal_cancelled=false; post-exit settlement failed: {cause_message}; {retained}; supported recovery: {recovery}",
                goal_id
                    .map(|goal_id| format!(" for Goal {goal_id}"))
                    .unwrap_or_default()
            ),
        )
    }
}
