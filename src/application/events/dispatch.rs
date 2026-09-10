mod actions;
mod context;
use super::{EventInvocation, FileEventService, InvocationContext, InvocationState};
use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::work_items::FileWorkItemService;
use crate::application::workflow::WorkflowEngine;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::automation::{
    AutomationConfig, BindingMode, CUSTOM_EVENT_ID, EventDefinition, EventKind, custom_event,
    valid_id,
};
use crate::model::workflow::GoalStatus;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
static SCAN_CURSOR: AtomicUsize = AtomicUsize::new(0);

use super::waiting::WaitReason;
use crate::application::workflow::engine::admission::{ExecutionReservation, reserve};
static DISPATCH: Mutex<()> = Mutex::new(());

impl FileEventService {
    pub(crate) fn manual_skill_event(
        &self,
        config: &AutomationConfig,
        skill_id: &str,
        node: &str,
    ) -> RefineResult<EventDefinition> {
        let skill = config
            .skills
            .get(skill_id)
            .ok_or_else(|| RefineError::NotFound(format!("Skill {skill_id}")))?;
        let selected: Vec<_> = config
            .events
            .values()
            .filter(|event| event.kind == EventKind::Custom)
            .flat_map(|event| {
                config
                    .bindings(event, node)
                    .into_iter()
                    .map(move |(binding, _)| (event, binding))
            })
            .filter(|(_, binding)| {
                binding.skill_id == skill_id && binding.mode != BindingMode::Context
            })
            .collect();
        if selected.len() != 1 {
            return Err(RefineError::InvalidInput("Manual launch requires one enabled Custom trigger for this Skill on the selected node".into()));
        }
        let mut event = custom_event();
        event.name = skill.name.clone();
        event.parameters = selected[0].0.parameters.clone();
        let mut binding = selected[0].1.clone();
        // Scope and override selection are already resolved. A manual Skill has
        // one result and no Goal action, regardless of workflow execution mode.
        binding.overrides = None;
        binding.scope.node_id = Some(node.into());
        binding.mode = BindingMode::Blocking;
        event.bindings.push(binding);
        Ok(event)
    }

    pub fn skill_inputs(&self, skill_id: &str, target: &Path) -> RefineResult<Value> {
        let context = self.manual_context(target, &json!({}))?;
        let config = self.config()?;
        let event = self.manual_skill_event(&config, skill_id, &context.node_id)?;
        Ok(
            json!({"revision":config.revision, "name":event.name, "parameters":self.launch_parameters(&config, &event, &context)?}),
        )
    }

    /// Interactive web runs reuse managed terminal lifecycle and transcripts.
    /// Resolve exactly the same selected Skill and typed inputs as headless runs.
    pub fn terminal_skill_prompt(
        &self,
        skill_id: &str,
        target: &Path,
        parameters: &Value,
    ) -> RefineResult<(String, Value)> {
        let mut context = self.manual_context(target, &json!({}))?;
        let config = self.config()?;
        let event = self.manual_skill_event(&config, skill_id, &context.node_id)?;
        let inputs = serde_json::from_value(parameters.clone())
            .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        let bindings = self.resolve_bindings(&config, &event, &mut context, &inputs)?;
        let pinned = bindings.first().ok_or_else(|| {
            RefineError::InvalidInput("This Skill has no enabled Custom trigger".into())
        })?;
        let prompt = format!(
            "Run this standalone Skill in the selected project. Follow its instructions and report what you did. This run is independent of Goal workflows; create or change Goals only when the Skill instructions or the user request authorize it.\n\nSkill: {}\n{}\n\nParameters:\n{}\n\nSystem context:\n{}",
            pinned.skill.name,
            pinned.skill.prompt,
            json!(pinned.parameters),
            context.data["system"]
        );
        Ok((
            prompt,
            json!({"skill_id": skill_id, "skill_name": pinned.skill.name, "skill_configuration_revision": config.revision, "skill_parameters": pinned.parameters, "node_id": context.node_id}),
        ))
    }

    pub fn trigger_skill(
        &self,
        skill_id: &str,
        target: &Path,
        body: &Value,
    ) -> RefineResult<EventInvocation> {
        if !valid_id(skill_id) {
            return Err(RefineError::InvalidInput("invalid Skill ID".into()));
        }
        let object = body
            .as_object()
            .ok_or_else(|| RefineError::InvalidInput("Skill input must be an object".into()))?;
        if object
            .keys()
            .any(|key| !["parameters", "node_id", "request_id"].contains(&key.as_str()))
        {
            return Err(RefineError::InvalidInput("Manual Skills take parameters, node_id, and request_id; they run independently of Goals".into()));
        }
        for key in ["node_id", "request_id"] {
            if body
                .get(key)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(RefineError::InvalidInput(format!("{key} must be a string")));
            }
        }
        let mut context = self.manual_context(target, body)?;
        let inputs: BTreeMap<String, Value> =
            serde_json::from_value(body.get("parameters").cloned().unwrap_or_else(|| json!({})))
                .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        let request_id = body
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if !valid_id(&request_id) {
            return Err(RefineError::InvalidInput("invalid request_id".into()));
        }
        context
            .metadata
            .insert("requested_parameters".into(), json!(inputs));
        context
            .metadata
            .insert("manual_skill_id".into(), json!(skill_id));
        let occurrence = format!("manual-skill:{skill_id}:{request_id}");
        let id = super::execution::stable_id(&format!("{occurrence}:{CUSTOM_EVENT_ID}"));
        if self.invocation_path(&id)?.exists() {
            let existing = self.invocation(&id)?;
            if existing.context.node_id != context.node_id
                || existing.context.metadata.get("requested_parameters")
                    != context.metadata.get("requested_parameters")
            {
                return Err(RefineError::Conflict(
                    "request_id already identifies a different Skill request".into(),
                ));
            }
            return Ok(existing);
        }
        let config = self.config()?;
        let event = self.manual_skill_event(&config, skill_id, &context.node_id)?;
        self.prepare_pinned(&config, &event, context, inputs, &occurrence)
    }

    pub fn trigger(
        &self,
        event_id: &str,
        target_root: &Path,
        body: &Value,
    ) -> RefineResult<EventInvocation> {
        let event = self
            .config()?
            .events
            .get(event_id)
            .cloned()
            .ok_or_else(|| RefineError::NotFound(format!("Event {event_id}")))?;
        if event.kind != EventKind::Custom {
            return Err(RefineError::InvalidInput(
                "system Events are emitted by Refine; create a custom Event for manual launch"
                    .into(),
            ));
        }
        let object = body
            .as_object()
            .ok_or_else(|| RefineError::InvalidInput("Event input must be an object".into()))?;
        if object
            .keys()
            .any(|k| !["parameters", "goal_id", "node_id", "request_id"].contains(&k.as_str()))
        {
            return Err(RefineError::InvalidInput(
                "unknown Event trigger field".into(),
            ));
        }
        for key in ["goal_id", "node_id", "request_id"] {
            if body
                .get(key)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(RefineError::InvalidInput(format!("{key} must be a string")));
            }
        }
        let mut context = self.manual_context(target_root, body)?;
        if event.on_success.is_some() && context.goal_id.is_none() {
            return Err(RefineError::InvalidInput(
                "Select a Goal for this Event's success action".into(),
            ));
        }
        let inputs: BTreeMap<String, Value> =
            serde_json::from_value(body.get("parameters").cloned().unwrap_or_else(|| json!({})))
                .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        let request_id = body
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if !valid_id(&request_id) {
            return Err(RefineError::InvalidInput("invalid request_id".into()));
        }
        context
            .metadata
            .insert("requested_parameters".into(), json!(inputs));
        let id = super::execution::stable_id(&format!("manual:{request_id}:{event_id}"));
        if self.invocation_path(&id)?.exists() {
            let existing = self.invocation(&id)?;
            if existing.context.node_id != context.node_id
                || existing.context.goal_id != context.goal_id
                || existing.context.metadata.get("requested_parameters")
                    != context.metadata.get("requested_parameters")
            {
                return Err(RefineError::Conflict(
                    "request_id already identifies a different Event request".into(),
                ));
            }
            return Ok(existing);
        }
        self.prepare(event_id, context, inputs, &format!("manual:{request_id}"))
    }

    /// Called by the existing workflow worker; only pending records are inspected.
    pub fn dispatch_pending(&self, target_root: &Path) -> RefineResult<usize> {
        self.dispatch_pending_limit(target_root, 32)
    }

    pub(crate) fn dispatch_pending_limit(
        &self,
        target_root: &Path,
        limit: usize,
    ) -> RefineResult<usize> {
        if limit == 0 {
            return Ok(0);
        }
        let _dispatch = match DISPATCH.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(0),
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        };
        let runtime = self.runtime()?;
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, runtime)
            .active_node_id()?;
        self.repair_invocation_indexes(Some(&node))?;
        let directory = self.refine_dir.join("automation/pending").join(&node);
        if !directory.exists() {
            return Ok(0);
        }
        let engine = WorkflowEngine::with_target_root(runtime, target_root);
        let paused = engine.workflow_paused()?;
        let policy = engine.policy_for_refine_dir_and_node(&self.refine_dir, &node)?;
        let mut launched = 0;
        let mut paths = std::fs::read_dir(directory)
            .map_err(|e| RefineError::Io(e.to_string()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.sort();
        if paths.is_empty() {
            return Ok(0);
        }
        let offset = SCAN_CURSOR.load(Ordering::Relaxed) % paths.len();
        paths.rotate_left(offset);
        for path in paths.into_iter().take(256) {
            SCAN_CURSOR.fetch_add(1, Ordering::Relaxed);
            if path.extension().and_then(|p| p.to_str()) != Some("json") {
                continue;
            }
            let record: Value = match read_json(&path) {
                Ok(record) => record,
                Err(_) if !path.exists() => continue,
                Err(_) => continue,
            };
            if record.get("node_id").and_then(Value::as_str) != Some(&node) {
                continue;
            }
            let Some(id) = record.get("id").and_then(Value::as_str) else {
                continue;
            };
            let invocation = match self.invocation(id) {
                Ok(value) => value,
                Err(_) => continue,
            };
            // Active workflow phases own these invocations and their capacity themselves.
            if (invocation.context.workflow_revision.is_some() && !invocation.state.terminal())
                || (invocation.state.terminal() && !invocation.success_action_ready())
            {
                continue;
            }
            if crate::application::workflow::engine::admission::reservations(runtime)
                .iter()
                .any(|r| r.invocation_id.as_deref() == Some(id))
            {
                continue;
            }
            if paused {
                self.record_wait(id, Some(WaitReason::Paused))?;
                continue;
            }
            match crate::infrastructure::storage::workspace::WorkspaceLease::acquire(
                &invocation.context.cwd,
            ) {
                Ok(lease) => drop(lease),
                Err(RefineError::Degraded(message))
                    if message.starts_with("workspace is in use") =>
                {
                    self.record_wait(id, Some(WaitReason::WorkspaceBusy))?;
                    continue;
                }
                Err(error) => {
                    eprintln!("refine Skill admission: {error}");
                    continue;
                }
            }
            let key = format!("{}:{}:{id}", runtime.display(), self.refine_dir.display());
            let Some(lease) = reserve(
                &engine,
                &policy,
                key,
                ExecutionReservation {
                    runtime: runtime.into(),
                    invocation_id: Some(id.into()),
                    goal_id: None,
                    node: node.clone(),
                    provider: invocation.context.provider.clone(),
                    target: target_root.display().to_string(),
                },
            )?
            else {
                self.record_wait(id, Some(WaitReason::Capacity))?;
                continue;
            };
            self.record_wait(id, None)?;
            let service = self.clone();
            std::thread::spawn(move || {
                let _lease = lease;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    service.execute(&invocation.id, || {
                        service.validate_manual_authority(&invocation)
                    })
                }))
                .unwrap_or_else(|_| Err(RefineError::Degraded("Skill worker panicked".into())));
                match result {
                    Ok(mut result) if result.success_action_ready() => {
                        if let Err(error) = service.apply_success_action(&mut result) {
                            result.state = InvocationState::Error;
                            result.error = Some(error.to_string());
                            let _ = service.save_invocation(&result);
                        }
                    }
                    Ok(_) => {}
                    Err(RefineError::Degraded(message))
                        if message.starts_with("workspace is in use") =>
                    {
                        let _ =
                            service.record_wait(&invocation.id, Some(WaitReason::WorkspaceBusy));
                    }
                    Err(error) => {
                        if let Ok(mut result) = service.invocation(&invocation.id)
                            && !result.state.terminal()
                        {
                            result.state = InvocationState::Error;
                            result.error = Some(error.to_string());
                            let _ = service.save_invocation(&result);
                        }
                    }
                }
            });
            launched += 1;
            if launched >= limit {
                break;
            }
        }
        Ok(launched)
    }

    pub fn startup_ready(&self, target_root: &Path, startup_id: &str) -> RefineResult<()> {
        let config = self.config()?;
        let context = self.manual_context(target_root, &json!({}))?;
        for event in config.events.values().filter(|e| {
            e.enabled
                && e.source.as_deref() == Some("node.startup.ready")
                && e.scope.applies(&context.node_id)
        }) {
            if config.bindings(event, &context.node_id).is_empty() {
                continue;
            }
            if let Err(error) = self.prepare_pinned(
                &config,
                event,
                context.clone(),
                BTreeMap::new(),
                &format!("startup:{}:{startup_id}", context.node_id),
            ) {
                write_json(
                    &self.refine_dir.join("automation/startup-error.json"),
                    &json!({"event_id": event.id, "startup_id": startup_id, "error": error.to_string()}),
                )?;
            }
        }
        Ok(())
    }
}
