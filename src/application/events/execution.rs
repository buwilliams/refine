use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::FileEventService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::model::automation::*;

pub use super::records::{EventInvocation, InvocationContext, InvocationState, PinnedBinding};

pub use super::context::goal_context;
use super::parameters::field;
pub use super::parameters::resolve_parameters;

pub fn stable_id(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl FileEventService {
    pub fn prepare(
        &self,
        event_id: &str,
        context: InvocationContext,
        inputs: BTreeMap<String, Value>,
        occurrence: &str,
    ) -> RefineResult<EventInvocation> {
        let config = self.config()?;
        let event = config
            .events
            .get(event_id)
            .ok_or_else(|| RefineError::NotFound(format!("Event {event_id}")))?;
        self.prepare_pinned(&config, event, context, inputs, occurrence)
    }

    pub fn prepare_pinned(
        &self,
        config: &AutomationConfig,
        event: &EventDefinition,
        mut context: InvocationContext,
        inputs: BTreeMap<String, Value>,
        occurrence: &str,
    ) -> RefineResult<EventInvocation> {
        if !valid_id(&context.node_id) {
            return Err(RefineError::InvalidInput(
                "invalid execution node ID".into(),
            ));
        }
        let id = stable_id(&format!("{occurrence}:{}", event.id));
        let error_context = context.clone();
        let prepared = with_record_lock(&self.refine_dir, &format!("event-{id}"), || {
            if self.invocation_path(&id)?.exists() {
                let existing = self.invocation(&id)?;
                if context.metadata.contains_key("requested_parameters")
                    && (existing.context.node_id != context.node_id
                        || existing.context.goal_id != context.goal_id
                        || existing.context.metadata.get("requested_parameters")
                            != context.metadata.get("requested_parameters"))
                {
                    return Err(RefineError::Conflict(
                        "request_id already identifies a different Event request".into(),
                    ));
                }
                return Ok(existing);
            }
            if !event.enabled || !event.scope.applies(&context.node_id) {
                return Err(RefineError::InvalidInput(
                    "Event is disabled or outside this node's scope".into(),
                ));
            }
            if event.on_success.is_some() && context.goal_id.is_none() {
                return Err(RefineError::InvalidInput(
                    "Event success action requires a Goal".into(),
                ));
            }
            let bindings = self.resolve_bindings(config, event, &mut context, &inputs)?;
            let invocation = EventInvocation {
                id: id.clone(),
                event: event.clone(),
                config_revision: config.revision,
                context,
                bindings,
                state: InvocationState::Pending,
                results: BTreeMap::new(),
                attempts: Vec::new(),
                created_at: now(),
                completed_at: None,
                error: None,
                action_applied: false,
            };
            self.save_invocation(&invocation)?;
            Ok(invocation)
        });
        if let Err(error) = &prepared
            && !self.invocation_path(&id)?.exists()
        {
            self.save_invocation(&EventInvocation {
                id,
                event: event.clone(),
                config_revision: config.revision,
                context: error_context,
                bindings: Vec::new(),
                state: InvocationState::Error,
                results: BTreeMap::new(),
                attempts: Vec::new(),
                created_at: now(),
                completed_at: Some(now()),
                error: Some(error.to_string()),
                action_applied: false,
            })?;
        }
        prepared
    }

    pub(crate) fn resolve_bindings(
        &self,
        config: &AutomationConfig,
        event: &EventDefinition,
        context: &mut InvocationContext,
        inputs: &BTreeMap<String, Value>,
    ) -> RefineResult<Vec<PinnedBinding>> {
        let effective = config.bindings(event, &context.node_id);
        let mut data = context.data.as_object().cloned().unwrap_or_default();
        let parameters = self.launch_parameters(config, event, context)?;
        let values = resolve_parameters(&parameters, inputs, &BTreeMap::new())?;
        data.insert("event".into(), json!(values));
        context.data = Value::Object(data);
        let mut bindings = Vec::new();
        for (binding, skill) in effective {
            let mut mapped: BTreeMap<String, Value> = values
                .iter()
                .filter(|(name, _)| skill.parameters.iter().any(|p| &p.name == *name))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            mapped.extend(binding.inputs.iter().filter_map(|(name, path)| {
                field(&context.data, path).map(|v| (name.clone(), v.clone()))
            }));
            let explicit = inputs
                .iter()
                .filter(|(name, _)| skill.parameters.iter().any(|p| &p.name == *name))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            bindings.push(PinnedBinding {
                binding: binding.clone(),
                skill: {
                    let mut snapshot = skill.clone();
                    snapshot.role = event.result_role().into();
                    snapshot
                },
                parameters: resolve_parameters(&skill.parameters, &explicit, &mapped)?,
            });
        }
        Ok(bindings)
    }

    pub fn launch_parameters(
        &self,
        config: &AutomationConfig,
        event: &EventDefinition,
        context: &InvocationContext,
    ) -> RefineResult<Vec<Parameter>> {
        let mut parameters = event.parameters.clone();
        for (binding, skill) in config.bindings(event, &context.node_id) {
            for parameter in &skill.parameters {
                if binding
                    .inputs
                    .get(&parameter.name)
                    .and_then(|path| field(&context.data, path))
                    .is_some()
                {
                    continue;
                }
                let mut parameter = parameter.clone();
                if let Some(name) = binding
                    .inputs
                    .get(&parameter.name)
                    .and_then(|p| p.strip_prefix("event."))
                {
                    parameter.name = name.into();
                }
                if let Some(existing) = parameters.iter_mut().find(|p| p.name == parameter.name) {
                    if existing.kind != parameter.kind || existing.choices != parameter.choices {
                        return Err(RefineError::InvalidInput(format!(
                            "conflicting parameter types for {}",
                            parameter.name
                        )));
                    }
                    existing.required |= parameter.required;
                    if existing.default.is_none() {
                        existing.default = parameter.default.clone();
                    } else if parameter.default.is_some() && existing.default != parameter.default {
                        return Err(RefineError::InvalidInput(format!(
                            "conflicting defaults for {}",
                            parameter.name
                        )));
                    }
                } else {
                    parameters.push(parameter);
                }
            }
        }
        Ok(parameters)
    }

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
        let result = self.execute_inner(id, launch_metadata, validate_authority);
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
            return Ok(invocation);
        }
        validate_authority()?;
        use crate::infrastructure::process::supervisor::operations::{
            FileOperationRegistry, OperationRegistry, OperationState,
        };
        let operations = FileOperationRegistry::new(runtime);
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
        self.save_invocation(&invocation)?;
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
                metadata.insert("event_invocation_id".into(), json!(id));
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
                    crate::application::agent_io::contracts::skill_result::result_contract(
                        id,
                        &pinned.binding.id,
                        &pinned.skill.role,
                    );
                let observational = pinned.skill.role == "plan"
                    || pinned.skill.role == "governance"
                    || invocation
                        .context
                        .data
                        .get("verification_only")
                        .and_then(Value::as_bool)
                        == Some(true);
                let role_instructions = if pinned.skill.role == "governance" {
                    "For failure, artifacts.violations must contain objects with stable rule_id and message fields; also fill recovery_analysis and recovery_round_prompt."
                } else {
                    ""
                };
                let authority = if invocation.context.goal_id.is_some() {
                    "Do not change Goal state, merge or push."
                } else {
                    "Perform only the actions authorized by the Skill instructions and inputs. Use supported Refine commands for Goal changes."
                };
                let prompt = format!(
                    "{}\n{role_instructions}\n\nAttached Skills:\n{}\n\nParameters:\n{}\n\nPinned context:\n{}\n\nRefine completion contract (supplied by the system):\n{}\nReturn one JSON object matching this contract. {authority} Identity fields must be copied exactly. Use outcome failure for findings and error for execution faults. Supply actual evidence; do not fabricate a pass. {}",
                    pinned.skill.prompt,
                    contexts,
                    json!(pinned.parameters),
                    invocation.context.data,
                    contract,
                    if observational {
                        "This invocation is observational: do not change files or Git state. Report findings and proposed check commands against the pinned candidate."
                    } else {
                        ""
                    }
                );
                super::completion::run(
                    self,
                    &mut invocation,
                    &pinned,
                    &prompt,
                    &contract,
                    &metadata,
                    Some(seconds("agent_idle_timeout_seconds", 900)).filter(|v| *v > 0),
                    observational,
                    &validate_authority,
                )
            })();
            match run {
                Ok(result) => {
                    let failed =
                        result.outcome == "error" && pinned.binding.mode == BindingMode::Blocking;
                    invocation.results.insert(pinned.binding.id.clone(), result);
                    self.save_invocation(&invocation)?;
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
        self.save_invocation(&invocation)?;
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

pub fn aggregate_state(invocation: &EventInvocation) -> InvocationState {
    let mut failed = false;
    for binding in invocation
        .bindings
        .iter()
        .filter(|b| b.binding.mode != BindingMode::Context)
    {
        match invocation
            .results
            .get(&binding.binding.id)
            .map(|r| r.outcome.as_str())
        {
            Some("success") => {}
            Some("failure") => failed = true,
            _ => return InvocationState::Error,
        }
    }
    if failed {
        InvocationState::Failed
    } else {
        InvocationState::Succeeded
    }
}

impl EventInvocation {
    /// Keep historical recorded state intact while exposing the outcome of all actual executions.
    pub fn execution_state(&self) -> InvocationState {
        if !self.state.terminal()
            || self.state == InvocationState::Cancelled
            || (self.state == InvocationState::Error
                && self.results.values().all(|r| r.outcome != "error"))
        {
            return self.state.clone();
        }
        aggregate_state(self)
    }

    pub(crate) fn execution_error(&self) -> RefineError {
        let message = self
            .error
            .clone()
            .unwrap_or_else(|| format!("Event {} execution failed", self.event.name));
        if self
            .results
            .values()
            .any(|r| r.artifacts["fault_kind"] == "output_contract")
        {
            RefineError::Serialization(message)
        } else {
            RefineError::Degraded(message)
        }
    }
}
