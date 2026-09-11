//! Surface-independent workflow decisions. A Skill invocation is optional provenance.
use super::*;
use serde_json::json;

impl FileWorkItemService {
    pub fn control_workflow(&self, id: &str, request: &WorkflowControl) -> RefineResult<Value> {
        self.control_workflow_operation(id, request, false)
    }

    pub(crate) fn control_workflow_operation(
        &self,
        id: &str,
        request: &WorkflowControl,
        integrate: bool,
    ) -> RefineResult<Value> {
        if request.reason.trim().is_empty()
            || request.request_id.trim().is_empty()
            || request.actor.trim().is_empty()
        {
            return Err(RefineError::InvalidInput(
                "reason, request_id and actor are required".into(),
            ));
        }
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        self.ensure_goal_owned(&current)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        let encoded =
            serde_json::to_value(request).map_err(|e| RefineError::Serialization(e.to_string()))?;
        if let Some(receipt) = goal["workflow_controls"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["request_id"] == request.request_id))
        {
            if receipt["request"] != encoded
                || receipt["integration_requested"].as_bool().unwrap_or(false) != integrate
            {
                return Err(RefineError::Conflict(
                    "request_id already identifies another workflow decision".into(),
                ));
            }
            return Ok(receipt.clone());
        }
        if goal["workflow_integration_control"]["state"] == "pending"
            && request.to != GoalStatus::Cancelled
        {
            return Err(RefineError::Conflict("An explicit integration is pending; reconcile its retained operation before another decision".into()));
        }
        if workflow_revision(&goal) != request.expected_revision {
            return Err(RefineError::Conflict(
                "Goal changed; refresh its workflow revision before deciding".into(),
            ));
        }
        if let Some(invocation) = &request.invocation_id {
            let run = crate::application::events::FileEventService::new(&self.refine_dir)
                .invocation(invocation)?;
            if run.context.goal_id.as_deref() != Some(id)
                || run.state.terminal()
                || run.context.node_id != self.active_node_id()?
                || run.context.data["goal"]["event_generation"] != goal["event_generation"]
                || (run.context.data["outcome"]["id"].is_string()
                    && (run.context.data["outcome"]["id"]
                        != goal["pending_workflow_outcome"]["id"]
                        || goal["pending_workflow_outcome"]["state"] != "pending"
                        || goal["pending_workflow_outcome"]["deadline_ms"]
                            .as_i64()
                            .unwrap_or(0)
                            <= chrono::Utc::now().timestamp_millis()))
            {
                return Err(RefineError::Conflict(
                    "Invocation no longer authorizes this Goal decision".into(),
                ));
            }
        }
        let from = current.goal.status;
        let recovery = matches!(
            request.to,
            GoalStatus::Plan
                | GoalStatus::Todo
                | GoalStatus::Backlog
                | GoalStatus::Failed
                | GoalStatus::Cancelled
        ) || (request.to == from && is_automated_status(&from));
        if !request.force && !recovery {
            validate_manual_goal_transition(&from, &request.to)?;
        }
        if !request.force && request.to == GoalStatus::Done {
            return Err(RefineError::InvalidInput("Use Goal approval to accept a reviewed candidate, or explicitly force status-only Done".into()));
        }
        let now = now_timestamp();
        let mut receipt = json!({"request_id":request.request_id,"request":encoded,"from":from,"to":request.to,
            "at":now,"forced":request.force,"integration_performed":false,"integration_requested":integrate,
            "source_round":goal["rounds"].as_array().map(Vec::len),
            "overridden_requirements":if request.force { json!(["transition_policy","workflow_evidence_gates"]) } else {json!([])},
            "retained_candidate":goal["candidate_commit"],"previous_attempt":goal["rounds"].as_array().and_then(|a|a.last()).and_then(|r|r.get("workflow_attempt_authority"))});
        let old_round = goal["rounds"].as_array().and_then(|a| a.last()).cloned();
        if request.to == GoalStatus::Plan {
            let prompt = if request.context.trim().is_empty() {
                request.reason.clone()
            } else {
                format!("{}\n\n{}", request.reason, request.context)
            };
            let mut round = new_round_value(&request.actor, "Refine", &prompt);
            round["workflow_control"] = receipt.clone();
            round["retained_candidate"] = goal["candidate_commit"].clone();
            goal["rounds"]
                .as_array_mut()
                .ok_or_else(|| RefineError::Conflict("Goal has no Rounds".into()))?
                .push(round);
            // Plan admission materializes the new Round through the existing Todo boundary.
            goal["workflow_requested_step"] = json!("plan");
            goal["status"] = json!("todo");
        } else {
            if (is_automated_status(&request.to) || request.to == GoalStatus::Todo)
                && old_round.is_none()
            {
                return Err(RefineError::Conflict(
                    "An executable step requires an authored Round".into(),
                ));
            }
            if matches!(request.to, GoalStatus::Quality | GoalStatus::Governance)
                && !goal["candidate_commit"].is_string()
            {
                return Err(RefineError::Conflict(
                    "This step requires a retained candidate".into(),
                ));
            }
            if let Some(round) = goal["rounds"].as_array_mut().and_then(|a| a.last_mut()) {
                let mut prior = round.clone();
                prior.as_object_mut().unwrap().remove("prior_attempts");
                if is_automated_status(&request.to) || request.to == GoalStatus::Todo {
                    let history = round
                        .as_object_mut()
                        .unwrap()
                        .entry("prior_attempts")
                        .or_insert(json!([]));
                    history.as_array_mut().unwrap().push(prior);
                    if request.to != GoalStatus::Governance {
                        for key in [
                            "quality_state",
                            "quality_message",
                            "quality_details",
                            "quality_checked_at",
                            "quality_candidate_commit",
                        ] {
                            round.as_object_mut().unwrap().remove(key);
                        }
                    }
                    for key in [
                        "rule_state",
                        "meta_rule_state",
                        "product_state",
                        "constitution_state",
                        "governance_message",
                        "governance_details",
                        "governance_checked_at",
                        "governance_candidate_commit",
                        "failure_category",
                        "failure_message",
                        "failure_at",
                    ] {
                        round.as_object_mut().unwrap().remove(key);
                    }
                    for key in [
                        "workflow_attempt_authority",
                        "event_results",
                        "gate_configurations",
                        "event_configuration",
                    ] {
                        round.as_object_mut().unwrap().remove(key);
                    }
                } else {
                    round
                        .as_object_mut()
                        .unwrap()
                        .remove("workflow_attempt_authority");
                }
            }
            goal["status"] = json!(request.to);
        }
        if integrate {
            goal["workflow_integration_control"] =
                json!({"request_id":request.request_id,"state":"pending"});
        }
        receipt["stopped_processes"] =
            json!(self.stop_goal_execution(id, request.invocation_id.as_deref())?);
        goal.as_object_mut()
            .unwrap()
            .entry("workflow_controls")
            .or_insert(json!([]))
            .as_array_mut()
            .unwrap()
            .push(receipt.clone());
        goal["workflow_context"] = json!(request.context);
        if let Some(pending) = goal.get_mut("pending_workflow_outcome") {
            pending["state"] = json!("redirected");
            pending["decision"] = receipt.clone();
        }
        if goal["status"].as_str() == Some(from.as_str()) {
            goal["event_generation"] = json!(goal["event_generation"].as_u64().unwrap_or(0) + 1);
        }
        goal["updated"] = json!(now);
        if request.force {
            crate::application::events::transitions::approve_exit(
                &self.refine_dir,
                &self.show_goal_detail(id)?,
                goal["status"].as_str().unwrap(),
            )?;
        }
        write_json_atomically(&path, &goal)?;
        Ok(receipt)
    }

    pub(crate) fn finish_pending_outcome(
        &self,
        id: &str,
        occurrence: &str,
        diagnostic: &str,
    ) -> RefineResult<()> {
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        self.ensure_goal_owned(&current)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        if goal["pending_workflow_outcome"]["id"] != occurrence
            || goal["pending_workflow_outcome"]["state"] != "pending"
        {
            return Ok(());
        }
        goal["pending_workflow_outcome"]["state"] = json!("failed");
        goal["pending_workflow_outcome"]["handler_diagnostic"] = json!(diagnostic);
        if !matches!(
            current.goal.status,
            GoalStatus::Done | GoalStatus::Cancelled
        ) {
            goal["status"] = json!("failed");
        }
        goal["updated"] = json!(now_timestamp());
        write_json_atomically(&path, &goal)
    }
}

impl FileWorkItemService {
    pub(crate) fn interrupt_workflow(&self, id: &str, message: &str) -> RefineResult<()> {
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        self.ensure_goal_owned(&current)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        if goal["workflow_integration_control"]["state"] == "pending" {
            let request_id = goal["workflow_integration_control"]["request_id"].clone();
            let result = json!({"state":"interrupted", "diagnostic":message,
                "at":now_timestamp(), "integration_state":"Inspect retained Git integration evidence before another integration decision"});
            goal["workflow_integration_control"]["state"] = json!("interrupted");
            goal["workflow_integration_control"]["result"] = result.clone();
            if let Some(receipt) = goal["workflow_controls"]
                .as_array_mut()
                .and_then(|receipts| {
                    receipts
                        .iter_mut()
                        .find(|receipt| receipt["request_id"] == request_id)
                })
            {
                receipt["integration_result"] = result;
            }
        }
        if goal["pending_workflow_outcome"]["state"] == "pending" {
            goal["pending_workflow_outcome"]["state"] = json!("failed");
            goal["pending_workflow_outcome"]["handler_diagnostic"] =
                json!("Error handling was interrupted; no handler was relaunched");
            goal["status"] = json!("failed");
            goal["updated"] = json!(now_timestamp());
            return write_json_atomically(&path, &goal);
        }
        if !crate::application::events::outcomes::prepare_error(
            &self.refine_dir,
            &mut goal,
            "interrupted",
            message,
        )? {
            goal["status"] = json!("failed");
        }
        if let Some(round) = goal["rounds"].as_array_mut().and_then(|r| r.last_mut()) {
            round["failure_category"] = json!("interrupted");
            round["failure_message"] = json!(message);
            round["failure_at"] = json!(now_timestamp());
        }
        goal["updated"] = json!(now_timestamp());
        write_json_atomically(&path, &goal)
    }
}

impl FileWorkItemService {
    pub(crate) fn settle_lifecycle_outcome(
        &self,
        id: &str,
        queued: &Value,
        fault: bool,
    ) -> RefineResult<()> {
        let source = queued["source"].as_str().unwrap_or_default();
        let parts = source.split('.').collect::<Vec<_>>();
        let ["workflow", status, edge] = parts.as_slice() else {
            return Ok(());
        };
        let terminal = matches!(*status, "done" | "cancelled" | "failed");
        if !fault && (!terminal || *edge != "enter") {
            return Ok(());
        }
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        self.ensure_goal_owned(&current)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        if goal["status"] != *status
            || goal["event_generation"] != queued["occurrence"]["generation"]
        {
            return Ok(());
        }
        // A pending transition owns settlement of its required Entry and Success
        // gates. Updating the Goal here would invalidate that same transition.
        if goal["pending_event_transition"]["state"] == "pending" {
            return Ok(());
        }
        let key = format!("{}:{source}", goal["event_generation"]);
        if goal["lifecycle_outcomes"]
            .as_object()
            .is_some_and(|outcomes| outcomes.contains_key(&key))
        {
            return Ok(());
        }
        goal.as_object_mut()
            .unwrap()
            .entry("lifecycle_outcomes")
            .or_insert(json!({}))[&key] = json!({
            "source":source,"state":if fault {"error"}else{"success"},"at":now_timestamp(),
        });
        if fault {
            if !crate::application::events::outcomes::prepare_error(
                &self.refine_dir,
                &mut goal,
                "lifecycle_event",
                &format!("Required {source} handler failed"),
            )? && !matches!(
                current.goal.status,
                GoalStatus::Done | GoalStatus::Cancelled
            ) {
                goal["status"] = json!("failed");
            }
        } else {
            // Terminal steps have no later worker departure to emit Success.
            // Retain their completed entry and enqueue the separate Success occurrence.
            let occurrence = json!({"id":uuid::Uuid::new_v4().to_string(),"generation":goal["event_generation"],
                "from":status,"to":status,"node_id":goal["node_id"].as_str().unwrap_or("default"),
                "round_idx":goal["rounds"].as_array().and_then(|rounds|rounds.len().checked_sub(1)),
                "at":now_timestamp(),"success":true});
            goal.as_object_mut()
                .unwrap()
                .entry("workflow_events")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap()
                .push(occurrence.clone());
            let node = goal["node_id"].as_str().unwrap_or("default");
            crate::infrastructure::storage::automation::write_json(
                &self
                    .refine_dir
                    .join("automation/occurrences")
                    .join(node)
                    .join(format!("{}.json", uuid::Uuid::new_v4())),
                &json!({"goal_path":current.goal.json_path,"source":format!("workflow.{status}.success"),
                    "node_id":node,"generation":goal["event_generation"],"previous_generation":goal["event_generation"],
                    "previous_round":occurrence["round_idx"],"occurrence":occurrence,"config":queued["config"],
                    "goal_context":crate::application::events::execution::goal_context(&goal),"candidate_commit":goal["candidate_commit"]}),
            )?;
        }
        goal["updated"] = json!(now_timestamp());
        write_json_atomically(&path, &goal)
    }

    fn stop_goal_execution(
        &self,
        goal_id: &str,
        caller: Option<&str>,
    ) -> RefineResult<Vec<String>> {
        use crate::infrastructure::process::subprocess::FileProcessSupervisor;
        let Some(runtime) = &self.active_node_root else {
            return Ok(Vec::new());
        };
        let node = self.active_node_id()?;
        let mut stopped = Vec::new();
        for root in [runtime.join("agents"), runtime.clone()] {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.capacity_processes()? {
                let Some(metadata) = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                else {
                    continue;
                };
                if metadata["goal_id"].as_str() != Some(goal_id)
                    || metadata["node_id"].as_str().unwrap_or("default") != node
                    || caller.is_some_and(|id| metadata["event_invocation_id"].as_str() == Some(id))
                {
                    continue;
                }
                // Registered group identity is revalidated by the supervisor; an
                // unproven exit aborts the decision before its status can change.
                supervisor
                    .terminate_and_confirm_exit(&process, std::time::Duration::from_secs(10))?;
                stopped.push(process.id);
            }
        }
        Ok(stopped)
    }
}

impl FileWorkItemService {
    pub(crate) fn finish_controlled_integration(
        &self,
        id: &str,
        request_id: &str,
        result: &RefineResult<Value>,
    ) -> RefineResult<Value> {
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        self.ensure_goal_owned(&current)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        if goal["workflow_integration_control"]["request_id"] != request_id {
            return Err(RefineError::Conflict(
                "Integration decision was superseded".into(),
            ));
        }
        let outcome = match result {
            Ok(value) => json!({"state":"succeeded","integration":value}),
            Err(error) => json!({"state":"failed","diagnostic":error.to_string()}),
        };
        goal["workflow_integration_control"]["state"] = outcome["state"].clone();
        goal["workflow_integration_control"]["result"] = outcome.clone();
        let receipt = goal["workflow_controls"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|receipt| receipt["request_id"] == request_id)
            .ok_or_else(|| RefineError::Conflict("Integration decision is missing".into()))?;
        receipt["integration_performed"] = json!(result.is_ok());
        receipt["integration_result"] = outcome;
        let receipt = receipt.clone();
        if current.goal.status == GoalStatus::Governance {
            if result.is_ok() {
                goal["status"] = json!("review");
            } else if !crate::application::events::outcomes::prepare_error(
                &self.refine_dir,
                &mut goal,
                "forced_integration",
                &result.as_ref().unwrap_err().to_string(),
            )? {
                goal["status"] = json!("failed");
            }
        }
        goal["updated"] = json!(now_timestamp());
        write_json_atomically(&path, &goal)?;
        Ok(receipt)
    }
}
