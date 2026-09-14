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
        let _templates = crate::application::templates::TemplateScope::pin(
            Some(&self.refine_dir),
            &mut invocation.context.metadata,
        )?;
        crate::application::templates::TemplateScope::set_values(
            crate::application::templates::TemplateScope::context_values(&invocation.context.data),
        );
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
        // Only a currently admitted workflow owner may continue an unfinished
        // occurrence. The workspace lease and authority checks exclude competing
        // coordinators; complete scope exit excludes surviving agent descendants.
        if launch_metadata.is_some()
            && invocation.context.goal_id.is_some()
            && invocation
                .event
                .source
                .as_deref()
                .is_some_and(|source| source.starts_with("workflow."))
            && (invocation.context.metadata.contains_key("started_bindings")
                || invocation
                    .context
                    .metadata
                    .contains_key("interrupted_resume"))
        {
            validate_authority()?;
            for root in [runtime.to_path_buf(), runtime.join("agents")] {
                let owner =
                    crate::infrastructure::process::subprocess::FileProcessSupervisor::new(root);
                for process in owner.capacity_processes()? {
                    let details: Value =
                        serde_json::from_str(process.details.as_deref().unwrap_or("{}"))
                            .map_err(|e| RefineError::Serialization(e.to_string()))?;
                    if details["event_invocation_id"] != id {
                        continue;
                    }
                    owner
                        .terminate_and_confirm_exit(&process, std::time::Duration::from_secs(2))?;
                }
            }
            validate_authority()?;
            if let Some(started) = invocation.context.metadata.remove("started_bindings") {
                invocation
                    .context
                    .metadata
                    .entry("interrupted_launches")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .ok_or_else(|| {
                        RefineError::Serialization("invalid interrupted launch history".into())
                    })?
                    .push(json!({"started_bindings":started,"reconciled_at":now()}));
            }
            invocation.context.metadata.remove("interrupted_resume");
            invocation
                .context
                .metadata
                .insert("resuming_work".into(), json!(true));
            self.save_accepted_invocation(&invocation, &validate_authority)?;
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
        use crate::application::templates::{TemplateScope, TemplateValue};
        let contexts = invocation
            .bindings
            .iter()
            .filter(|b| b.binding.mode == BindingMode::Context)
            .map(|b| {
                let mut values = TemplateScope::literals(&[
                    ("skill_name", &b.skill.name),
                    ("parameters", &json!(b.parameters).to_string()),
                ]);
                values.insert(
                    "skill".into(),
                    TemplateValue::Template(b.skill.prompt.clone()),
                );
                TemplateScope::render("context-skill", values)
            })
            .collect::<RefineResult<Vec<_>>>()?
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
                    // Lifecycle and standalone Skills need the same durable launch
                    // occurrence as workflow-owned agents, including the Skill that
                    // submits a redirect. Keep current recovered launch metadata when
                    // supplied; otherwise use the invocation's pinned Goal snapshot.
                    for (key, field) in [
                        ("workflow_step_generation", "event_generation"),
                        ("workflow_revision", "workflow_revision"),
                    ] {
                        if let Some(value) = invocation.context.data["goal"][field].as_u64() {
                            metadata.entry(key).or_insert(json!(value));
                        }
                    }
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
                let mut values = TemplateScope::literals(&[
                    ("attached_skills", &contexts),
                    ("parameters", &json!(pinned.parameters).to_string()),
                    ("context", &invocation.context.data.to_string()),
                    (
                        "execution",
                        &json!({"binding_id": pinned.binding.id, "role": pinned.skill.role})
                            .to_string(),
                    ),
                    ("completion_contract", &contract.to_string()),
                ]);
                values.insert(
                    "continuation".into(),
                    if invocation.context.metadata.get("resuming_work") == Some(&json!(true)) {
                        TemplateValue::Template("{{templates.workflow-continuation}}".into())
                    } else {
                        TemplateValue::Literal(String::new())
                    },
                );
                values.insert(
                    "observational".into(),
                    if observational {
                        TemplateValue::Template("{{templates.workflow-observational}}".into())
                    } else {
                        TemplateValue::Literal(String::new())
                    },
                );
                values.insert(
                    "skill".into(),
                    TemplateValue::Template(pinned.skill.prompt.clone()),
                );
                let template_id = if invocation
                    .event
                    .source
                    .as_deref()
                    .is_some_and(|source| source.starts_with("workflow."))
                {
                    "workflow"
                } else {
                    "supervised-skill"
                };
                let prompt = TemplateScope::render(template_id, values)?;
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
