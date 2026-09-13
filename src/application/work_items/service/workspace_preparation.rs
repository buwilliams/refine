//! Atomic binding of prepared work to the existing authored Round.
use super::*;
use serde_json::json;

impl FileWorkItemService {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bind_prepared_workspace(
        &self,
        id: &str,
        expected: WorkflowStepAuthority,
        status: &GoalStatus,
        branch: &str,
        target: &str,
        base: &str,
        candidate: Option<&str>,
        recovery: Option<&str>,
    ) -> RefineResult<WorkflowStepAuthority> {
        let (_lock, path, mut goal) = self.read_goal_value(id)?;
        workflow_attempts::require_current_step(id, goal.as_object().unwrap(), expected)?;
        if goal["status"] != status.as_str() {
            return Err(RefineError::Conflict(
                "Workflow changed during workspace preparation".into(),
            ));
        }
        if recovery.is_none()
            && goal["branch_name"] == branch
            && goal["base_commit"] == base
            && goal["target_branch"] == target
            && goal["candidate_commit"] == json!(candidate)
            && goal["rounds"][expected.round_idx]["workspace_branch"] == branch
        {
            return Ok(expected);
        }
        let now = now_timestamp();
        if let Some(reason) = recovery {
            let request = WorkflowControl {
                to: GoalStatus::Plan,
                reason: reason.into(),
                context: String::new(),
                expected_revision: workflow_revision(&goal),
                request_id: uuid::Uuid::new_v4().to_string(),
                force: false,
                actor: "refine".into(),
                invocation_id: None,
            };
            let previous = json!({"branch":goal["branch_name"],"base":goal["base_commit"],
                "candidate":goal["candidate_commit"],"target":goal["target_branch"],
                "step":status,"generation":goal["event_generation"],"reason":reason,"at":now});
            let round = &mut goal["rounds"][expected.round_idx];
            archive_round_for_retry(round, &GoalStatus::Plan)?;
            round
                .as_object_mut()
                .unwrap()
                .entry("workspace_recoveries")
                .or_insert(json!([]))
                .as_array_mut()
                .ok_or_else(|| {
                    RefineError::Serialization("Invalid workspace recovery history".into())
                })?
                .push(previous);
            goal["status"] = json!("plan");
            goal.as_object_mut()
                .unwrap()
                .entry("workflow_controls")
                .or_insert(json!([]))
                .as_array_mut()
                .ok_or_else(|| RefineError::Serialization("Invalid workflow controls".into()))?
                .push(
                    json!({"request_id":request.request_id,"request":request,"decision":true,
                    "from":status,"to":"plan","actor":"refine","reason":reason,"at":now,
                    "source_round":expected.round_idx + 1,"forced":false,"recovery":true}),
                );
        }
        goal["rounds"][expected.round_idx]["workspace_branch"] = json!(branch);
        goal["branch_name"] = json!(branch);
        goal["target_branch"] = json!(target);
        goal["base_commit"] = json!(base);
        goal["candidate_commit"] = json!(candidate);
        goal["updated"] = json!(now);
        write_json_atomically(&path, &goal)?;
        let mut reader = self.clone();
        reader.execution_occurrence = None;
        let observed = WorkflowStepAuthority::from_goal(&reader.show_goal_detail(id)?)?;
        // Metadata binding does not replace the originating attempt receipt.
        // As with an ordinary phase advance, only the occurrence can change.
        Ok(WorkflowStepAuthority {
            generation: observed.generation,
            ..expected
        })
    }
}
