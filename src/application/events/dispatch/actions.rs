//! Success actions and cancellation retain existing shared lifecycle authority.
use super::*;

impl FileEventService {
    pub(in super::super) fn apply_success_action(
        &self,
        invocation: &mut EventInvocation,
    ) -> RefineResult<()> {
        let Some(action) = invocation.event.on_success.clone() else {
            return Ok(());
        };
        if invocation.action_applied {
            return Ok(());
        }
        let id = invocation.context.goal_id.clone().ok_or_else(|| {
            RefineError::InvalidInput("Event success action requires a Goal".into())
        })?;
        with_record_lock(&self.refine_dir, &id, || {
            let work = FileWorkItemService::new(&self.refine_dir);
            let detail = work.show_goal_detail(&id)?;
            if detail
                .get("event_actions")
                .is_some_and(|receipts| receipts.get(&invocation.id).is_some())
            {
                invocation.action_applied = true;
                return self.save_invocation(invocation);
            }
            let required = super::super::execution::BlockingInvocation::pin(invocation)
                .into_iter()
                .collect::<Vec<_>>();
            self.settle_blocking(&required, |validation| {
                validation?;
                if invocation.bindings.iter().any(|b| b.binding.mode != BindingMode::Context) {
                    invocation.context.validate_workspace(&self.refine_dir)?;
                }
                self.validate_manual_authority(invocation)?;
                if let Some(occurrence) = invocation.context.data.get("occurrence")
                    && occurrence.get("generation") != detail.get("event_generation")
                {
                    return Err(RefineError::Conflict(
                        "Goal moved beyond this Event occurrence; no lifecycle action was applied"
                            .into(),
                    ));
                }
                let depth = if invocation.event.kind == EventKind::Custom {
                    1
                } else {
                    detail["event_action_depth"].as_u64().unwrap_or(0) + 1
                };
                if depth > 16 {
                    return Err(RefineError::Conflict("Event lifecycle action chain exceeded 16 actions; a new manual trigger is required".into()));
                }
                let current = work.show_goal_summary(&id)?;
                let applied = super::super::transitions::with_action(
                    json!({"invocation_id":invocation.id, "action": action, "depth": depth, "at":chrono::Utc::now().to_rfc3339()}),
                    || {
                        match action.as_str() {
                            "start" => {
                                work.start_goal_workflow(&id)?;
                            }
                            "accept" => {
                                work.transition_goal_status(&id, GoalStatus::Done)?;
                            }
                            "retry" => {
                                if !matches!(
                                    current.goal.status,
                                    GoalStatus::Failed | GoalStatus::Cancelled
                                ) {
                                    return Err(RefineError::Conflict(
                                        "retry requires Failed or Cancelled".into(),
                                    ));
                                }
                                work.transition_goal_status(&id, GoalStatus::Todo)?;
                            }
                            "reopen" => {
                                work.undo_goal_summary(&id)?;
                            }
                            _ => {
                                return Err(RefineError::InvalidInput(
                                    "unsupported success action".into(),
                                ));
                            }
                        }
                        Ok(())
                    },
                );
                if !matches!(&applied, Err(RefineError::Conflict(message)) if message.starts_with(super::super::transitions::PENDING))
                {
                    applied?;
                }
                invocation.action_applied = true;
                self.save_invocation(invocation)
            })
        })
    }

    pub(crate) fn cancel_goal_invocations(&self, goal_id: &str) -> RefineResult<()> {
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, self.runtime()?)
            .active_node_id()?;
        let directory = self.refine_dir.join("automation/pending").join(node);
        if !directory.exists() {
            return Ok(());
        }
        let goal = FileWorkItemService::new(&self.refine_dir).show_goal_detail(goal_id)?;
        for entry in std::fs::read_dir(directory).map_err(|e| RefineError::Io(e.to_string()))? {
            let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
            if path.extension().and_then(|p| p.to_str()) != Some("json") {
                continue;
            }
            let record: Value = match read_json(&path) {
                Ok(record) => record,
                Err(_) if !path.exists() => continue,
                Err(e) => return Err(e),
            };
            if let Some(id) = record["id"].as_str() {
                let invocation = self.invocation(id)?;
                if invocation.context.goal_id.as_deref() == Some(goal_id)
                    && invocation.context.data["goal"].get("event_generation")
                        != goal.get("event_generation")
                {
                    self.cancel_invocation(id)?;
                }
            }
        }
        Ok(())
    }

    pub fn cancel_invocation(&self, id: &str) -> RefineResult<EventInvocation> {
        with_record_lock(&self.refine_dir, &format!("event-{id}"), || {
            let mut invocation = self.invocation(id)?;
            if invocation.state.terminal() {
                return Ok(invocation);
            }
            invocation.state = InvocationState::Cancelled;
            invocation.completed_at = Some(chrono::Utc::now().to_rfc3339());
            self.save_invocation(&invocation)?;
            if let Some(operation_id) = invocation
                .context
                .metadata
                .get("event_operation_id")
                .and_then(Value::as_str)
            {
                use crate::infrastructure::process::supervisor::operations::{
                    FileOperationRegistry, OperationRegistry,
                };
                FileOperationRegistry::new(self.runtime()?).cancel(operation_id)?;
            }
            self.terminate_invocation_processes(id)?;
            Ok(invocation)
        })
    }

    pub(in super::super) fn terminate_invocation_processes(&self, id: &str) -> RefineResult<()> {
        use crate::infrastructure::process::subprocess::FileProcessSupervisor;
        let supervisor = FileProcessSupervisor::new(self.runtime()?);
        for process in supervisor.list()? {
            let metadata = process
                .details
                .as_deref()
                .and_then(|v| serde_json::from_str::<Value>(v).ok());
            if metadata
                .as_ref()
                .and_then(|m| m.get("event_invocation_id"))
                .and_then(Value::as_str)
                == Some(id)
            {
                supervisor.request_termination(&process.id, "terminate")?;
            }
        }
        Ok(())
    }
}
