//! Supervised execution in the admitted occurrence workspace.
use super::*;

impl FileEventService {
    /// Execute serially in one existing checkout. The authority callback runs before each
    /// launch, repair, accepted response, and final settlement.
    pub fn execute(
        &self,
        id: &str,
        validate_authority: impl Fn() -> RefineResult<()>,
    ) -> RefineResult<EventInvocation> {
        self.execute_with_metadata(id, None, validate_authority)
    }

    pub(crate) fn execute_with_metadata(
        &self,
        id: &str,
        launch_metadata: Option<&serde_json::Map<String, Value>>,
        validate_authority: impl Fn() -> RefineResult<()>,
    ) -> RefineResult<EventInvocation> {
        let result = self.execute_inner(id, launch_metadata, &validate_authority);
        if let Err(error) = &result {
            let busy = matches!(error, RefineError::Degraded(message) if message.starts_with("workspace is in use"));
            if !busy
                && let Ok(mut invocation) = self.invocation(id)
                && !invocation.state.terminal()
            {
                invocation.state = InvocationState::Error;
                invocation.completed_at = Some(now());
                invocation.error = Some(error.to_string());
                self.save_invocation(&invocation)?;
                if let Some(operation_id) = invocation
                    .context
                    .metadata
                    .get("event_operation_id")
                    .and_then(Value::as_str)
                {
                    use crate::infrastructure::process::supervisor::operations::{
                        FileOperationRegistry, OperationState,
                    };
                    let _ = FileOperationRegistry::new(self.runtime()?).finish_with_result(
                        operation_id,
                        OperationState::Failed,
                        json!({"event_invocation_id":id,"error":error.to_string()}),
                    );
                }
            }
        }
        result
    }

    fn execute_inner(
        &self,
        id: &str,
        launch_metadata: Option<&serde_json::Map<String, Value>>,
        validate_authority: impl Fn() -> RefineResult<()>,
    ) -> RefineResult<EventInvocation> {
        let mut invocation = self.invocation(id)?;
        if invocation.state.terminal() {
            self.validate_retained_invocation(&invocation)?;
            return Ok(invocation);
        }
        self.validate_lifecycle(&invocation)?;
        if invocation
            .bindings
            .iter()
            .any(|b| b.binding.mode != BindingMode::Context)
        {
            invocation.context.validate_workspace(&self.refine_dir)?;
        }
        if invocation
            .bindings
            .iter()
            .all(|b| b.binding.mode == BindingMode::Context)
        {
            validate_authority()?;
            invocation.state = InvocationState::Succeeded;
            invocation.completed_at = Some(now());
            self.save_invocation(&invocation)?;
            return Ok(invocation);
        }
        let runtime = self.runtime()?;
        #[cfg(test)]
        if invocation.context.provider != "smoke-ai" {
            return Err(RefineError::InvalidInput(
                "Event tests require an explicit smoke-ai provider fixture".into(),
            ));
        }
        let _lease = crate::infrastructure::storage::workspace::WorkspaceLease::acquire(
            &invocation.context.cwd,
        )?;
        // Reload after the checkout lease: a concurrent caller may have completed this work.
        invocation = self.invocation(id)?;
        if invocation.state.terminal() {
            self.validate_retained_invocation(&invocation)?;
            return Ok(invocation);
        }
        validate_authority()?;
        use crate::infrastructure::process::supervisor::operations::{
            FileOperationRegistry, OperationRegistry, OperationState,
        };
        let operations = FileOperationRegistry::new(runtime);
        if invocation
            .context
            .metadata
            .contains_key("event_operation_id")
        {
            invocation
                .context
                .metadata
                .insert("interrupted_resume".into(), json!(true));
        }
        if let Some(previous) = invocation
            .context
            .metadata
            .get("event_operation_id")
            .and_then(Value::as_str)
        {
            // A recovered worker must settle the old launch window before replacing it.
            if operations.status(previous).is_ok() {
                operations.cancel(previous)?;
                self.terminate_invocation_processes(id)?;
                operations.ensure_cancellation_processes_exited(previous)?;
            }
        }
        let operation = with_record_lock(&self.refine_dir, &format!("event-{id}"), || {
            if self.invocation(id)?.state == InvocationState::Cancelled {
                return Err(RefineError::Conflict(
                    "Event invocation was cancelled".into(),
                ));
            }
            let operation = operations.register_with_request(
                "event",
                json!({"event_invocation_id": id, "target_root": invocation.context.target_root}),
            )?;
            invocation
                .context
                .metadata
                .insert("event_operation_id".into(), json!(operation.id));
            self.save_invocation(&invocation)?;
            Ok(operation)
        })?;
        invocation.state = InvocationState::Running;
        self.save_accepted_invocation(&invocation, &validate_authority)?;
        use crate::infrastructure::process::supervisor::config::{
            ConfigService, FileSettingsService,
        };
        let settings = FileSettingsService::with_active_root(&self.refine_dir, runtime).load()?;
        let seconds = |key: &str, default: u64| {
            settings
                .get(key)
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|v| v.parse().ok()))
                })
                .unwrap_or(default)
        };
        let contexts = invocation
            .bindings
            .iter()
            .filter(|b| b.binding.mode == BindingMode::Context)
            .map(|b| {
                format!(
                    "{}\n{}\nParameters: {}",
                    b.skill.name,
                    b.skill.prompt,
                    json!(b.parameters)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        for pinned in invocation.bindings.clone() {
            if pinned.binding.mode == BindingMode::Context
                || invocation.results.contains_key(&pinned.binding.id)
            {
                continue;
            }
            let run = (|| -> RefineResult<SkillResult> {
                validate_authority()?;
                let mut metadata = invocation.context.metadata.clone();
                // The occurrence remains pinned. A recovered worker supplies its current
                // transient claim and parent operation for this particular process attempt.
                if let Some(current) = launch_metadata {
                    metadata.extend(current.clone());
                }
                invocation.context.process_workspace(&mut metadata);
                metadata.insert("event_invocation_id".into(), json!(id));
                metadata.insert(
                    "agent_hard_cap_millis".into(),
                    json!(seconds("agent_hard_cap_seconds", 7200).saturating_mul(1000)),
                );
                metadata.insert(
                    "agent_idle_timeout_millis".into(),
                    json!(seconds("agent_idle_timeout_seconds", 900).saturating_mul(1000)),
                );
                if let Some(goal_id) = &invocation.context.goal_id {
                    metadata.insert("goal_id".into(), json!(goal_id));
                }
                metadata.insert("skill_id".into(), json!(pinned.skill.id));
                metadata.insert("node_id".into(), json!(invocation.context.node_id));
                metadata.insert(
                    "target_app_id".into(),
                    json!(invocation.context.target_root),
                );
                metadata.insert(
                    "completion_timeout_seconds".into(),
                    json!(seconds("agent_hard_cap_seconds", 7200)),
                );
                let contract =
                    crate::application::agent_io::contracts::skill_result::report_contract();
                let observational = invocation
                    .context
                    .data
                    .get("verification_only")
                    .and_then(Value::as_bool)
                    == Some(true);
                let authority = "Follow the Skill instructions and current user authorization. Use supported Refine commands for Goal changes. Preserve confirmation boundaries and retained work. A workflow change supersedes this invocation; its old result cannot advance the new work.";
                let prompt = format!(
                    "{}\n\nAttached Skills:\n{}\n\nParameters:\n{}\n\nPinned context:\n{}\n\nSkill execution:\n{}\n\nRefine completion contract (supplied by the system):\n{}\nReturn one JSON object matching this contract. {authority} Refine attaches invocation, binding, and role identity to your response; do not include identity fields. Use outcome failure for unresolved findings after completing the authorized corrective work, and error for execution faults. Corrected findings do not require a failure outcome. Use your judgment to decide when to stop and which outcome to report. The summary, evidence, and artifacts fields are optional context; no checklist, test commands, supporting evidence, or recovery proposal is required by Refine. {}",
                    pinned.skill.prompt,
                    contexts,
                    json!(pinned.parameters),
                    invocation.context.data,
                    json!({"binding_id": pinned.binding.id, "role": pinned.skill.role}),
                    contract,
                    if observational {
                        "This invocation is observational: do not change files or Git state. Report your decision about the current work."
                    } else {
                        ""
                    }
                );
                let pinned_invocation = invocation.clone();
                let validate = || {
                    validate_authority()?;
                    self.validate_lifecycle(&pinned_invocation)?;
                    pinned_invocation
                        .context
                        .validate_workspace(&self.refine_dir)
                };
                super::super::completion::run(
                    self,
                    &mut invocation,
                    &pinned,
                    &prompt,
                    &contract,
                    &metadata,
                    Some(seconds("agent_idle_timeout_seconds", 900)).filter(|v| *v > 0),
                    observational,
                    &validate,
                )
            })();
            match run {
                Ok(result) => {
                    let failed =
                        result.outcome == "error" && pinned.binding.mode == BindingMode::Blocking;
                    invocation.results.insert(pinned.binding.id.clone(), result);
                    self.save_accepted_invocation(&invocation, &validate_authority)?;
                    if failed {
                        break;
                    }
                }
                Err(error) => {
                    invocation.results.insert(pinned.binding.id.clone(), SkillResult { invocation_id: id.into(), binding_id: pinned.binding.id.clone(), role: pinned.skill.role.clone(), outcome: "error".into(), summary: error.to_string(), evidence: Vec::new(), artifacts: json!({"fault_kind": if matches!(error, RefineError::Serialization(_)) { "output_contract" } else { "execution" }}) });
                    invocation.error = Some(error.to_string());
                    self.save_invocation(&invocation)?;
                    if pinned.binding.mode == BindingMode::Blocking {
                        break;
                    }
                }
            }
        }
        validate_authority()?;
        invocation.state = aggregate_state(&invocation);
        invocation.completed_at = Some(now());
        self.save_accepted_invocation(&invocation, &validate_authority)?;
        operations.finish_with_result(
            &operation.id,
            if invocation.state == InvocationState::Succeeded {
                OperationState::Succeeded
            } else {
                OperationState::Failed
            },
            json!({"event_invocation_id": id, "state": invocation.state}),
        )?;
        Ok(invocation)
    }
}
