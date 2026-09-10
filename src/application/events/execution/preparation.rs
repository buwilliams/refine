//! Input validation and durable invocation preparation.
use super::*;

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
        let lifecycle_goal = lifecycle::applicable(event, &context)
            .then(|| context.goal_id.clone())
            .flatten();
        // Lifecycle validation may recheck a workflow claim under the Goal lock.
        // Keep the same Goal -> invocation lock order as creation and acceptance.
        let prepare = || {
            with_record_lock(&self.refine_dir, &format!("event-{id}"), || {
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
                    if existing
                        .context
                        .lifecycle
                        .as_ref()
                        .is_none_or(|owner| owner.launch_ready)
                        || existing.state.terminal()
                    {
                        self.validate_retained_invocation(&existing)?;
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
                let mut bindings = self.resolve_bindings(config, event, &mut context, &inputs)?;
                if bindings
                    .iter()
                    .any(|binding| binding.binding.mode != BindingMode::Context)
                {
                    if lifecycle::applicable(event, &context) {
                        self.pin_lifecycle_intent(event, &mut context, &id, occurrence)?;
                        context.lifecycle.as_mut().unwrap().inputs = inputs.clone();
                    } else {
                        context.admit_workspace(&self.refine_dir)?;
                        context.resolve_workspace_parameters(&mut bindings, &inputs)?;
                    }
                }
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
            })
        };
        let prepared = if let Some(goal_id) = lifecycle_goal {
            with_record_lock(&self.refine_dir, &goal_id, prepare)
        } else {
            prepare()
        };
        let prepared = prepared.and_then(|mut invocation| {
            if invocation.context.lifecycle.is_some()
                && !invocation.state.terminal()
                && let Err(error) = self.materialize_lifecycle(&mut invocation)
            {
                if !self.invocation(&id)?.state.terminal() {
                    invocation.state = InvocationState::Error;
                    invocation.completed_at = Some(now());
                    invocation.error = Some(error.to_string());
                    self.save_invocation(&invocation)?;
                }
                return Err(error);
            }
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
}
