use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::FileEventService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{HostAgentProviderService, ProviderInvocation};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::automation::*;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InvocationContext {
    pub node_id: String,
    pub target_root: PathBuf,
    pub cwd: PathBuf,
    pub provider: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub round_idx: Option<usize>,
    #[serde(default)]
    pub workflow_revision: Option<u64>,
    #[serde(default)]
    pub candidate_commit: Option<String>,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Error,
    Cancelled,
}

impl InvocationState {
    pub fn terminal(&self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PinnedBinding {
    pub binding: Binding,
    pub skill: Skill,
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EventInvocation {
    pub id: String,
    pub event: EventDefinition,
    pub config_revision: u64,
    pub context: InvocationContext,
    pub bindings: Vec<PinnedBinding>,
    pub state: InvocationState,
    pub results: BTreeMap<String, SkillResult>,
    pub attempts: Vec<Value>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub action_applied: bool,
}

/// Keep authored Goal context and accepted artifacts without recursively embedding
/// configuration snapshots, prior invocation ledgers, and their copies of the Goal.
pub fn goal_context(goal: &Value) -> Value {
    let mut goal = goal.clone();
    let strip = |value: &mut Value| {
        if let Some(object) = value.as_object_mut() {
            for key in [
                "agent_context",
                "event_configuration",
                "pending_event_transition",
                "event_results",
                "workflow_events",
                "event_actions",
            ] {
                object.remove(key);
            }
        }
    };
    strip(&mut goal);
    if let Some(rounds) = goal.get_mut("rounds").and_then(Value::as_array_mut) {
        for round in rounds {
            strip(round);
        }
    }
    goal
}

pub fn stable_id(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl FileEventService {
    pub fn invocation_path(&self, id: &str) -> RefineResult<PathBuf> {
        if !valid_id(id) {
            return Err(RefineError::InvalidInput("invalid invocation ID".into()));
        }
        Ok(self
            .refine_dir
            .join("automation/invocations")
            .join(format!("{id}.json")))
    }
    pub fn invocation(&self, id: &str) -> RefineResult<EventInvocation> {
        read_json(&self.invocation_path(id)?)
    }
    pub fn save_invocation(&self, invocation: &EventInvocation) -> RefineResult<()> {
        with_record_lock(
            &self.refine_dir,
            &format!("event-{}", invocation.id),
            || {
                let path = self.invocation_path(&invocation.id)?;
                if path.exists() {
                    let current = self.invocation(&invocation.id)?;
                    if current.state == InvocationState::Cancelled
                        && invocation.state != InvocationState::Cancelled
                    {
                        return Err(RefineError::Conflict(
                            "Event invocation was cancelled".into(),
                        ));
                    }
                    if current.state.terminal() && !invocation.state.terminal() {
                        return Err(RefineError::Conflict(
                            "Event invocation is already settled".into(),
                        ));
                    }
                }
                let journal = self
                    .refine_dir
                    .join("automation/index-updates")
                    .join(&invocation.context.node_id)
                    .join(format!("{}.json", invocation.id));
                write_json(&journal, &json!({"id": invocation.id}))?;
                write_json(&path, invocation)?;
                self.write_invocation_indexes(invocation)?;
                remove_if_present(&journal)
            },
        )
    }

    fn write_invocation_indexes(&self, invocation: &EventInvocation) -> RefineResult<()> {
        let history = self.refine_dir.join("automation/history").join(format!(
            "{}-{}.json",
            invocation.created_at.replace(':', "-"),
            invocation.id
        ));
        write_json(
            &history,
            &json!({"id":invocation.id, "event":{"id":invocation.event.id,"name":invocation.event.name,"kind":invocation.event.kind,"source":invocation.event.source},"config_revision":invocation.config_revision,"state":invocation.state,"created_at":invocation.created_at,"completed_at":invocation.completed_at,"goal_id":invocation.context.goal_id,"node_id":invocation.context.node_id,"error":invocation.error,"result_count":invocation.results.len()}),
        )?;
        if let Some(goal_id) = &invocation.context.goal_id {
            if !valid_id(goal_id) {
                return Err(RefineError::InvalidInput("invalid Event Goal ID".into()));
            }
            let goal_history = self
                .refine_dir
                .join("automation/goal-history")
                .join(goal_id)
                .join(history.file_name().expect("history name"));
            let summary: Value = read_json(&history)?;
            write_json(&goal_history, &summary)?;
        }
        let pending = self
            .refine_dir
            .join("automation/pending")
            .join(&invocation.context.node_id)
            .join(format!("{}.json", invocation.id));
        if invocation.state.terminal()
            && !(invocation.state == InvocationState::Succeeded
                && invocation.event.on_success.is_some()
                && !invocation.action_applied)
        {
            remove_if_present(&pending)?;
        } else {
            write_json(
                &pending,
                &json!({"id": invocation.id, "node_id": invocation.context.node_id}),
            )?;
        }
        Ok(())
    }

    /// Recover only interrupted index updates, never scan historical invocation blobs.
    pub(crate) fn repair_invocation_indexes(&self, node: Option<&str>) -> RefineResult<()> {
        let root = self.refine_dir.join("automation/index-updates");
        if !root.exists() {
            return Ok(());
        }
        let directories = if let Some(node) = node {
            vec![root.join(node)]
        } else {
            std::fs::read_dir(&root)
                .map_err(|e| RefineError::Io(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| RefineError::Io(e.to_string()))?
                .into_iter()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        };
        let mut remaining = 128;
        for directory in directories {
            if !directory.exists() {
                continue;
            }
            for entry in std::fs::read_dir(directory)
                .map_err(|e| RefineError::Io(e.to_string()))?
                .take(remaining)
            {
                let journal = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
                if journal.extension().and_then(|v| v.to_str()) != Some("json") {
                    continue;
                }
                let id = journal
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| {
                        RefineError::Serialization("missing invocation index ID".into())
                    })?;
                self.invocation_path(id)?;
                with_record_lock(&self.refine_dir, &format!("event-{id}"), || {
                    if !journal.exists() {
                        return Ok(());
                    }
                    if self.invocation_path(id)?.exists() {
                        self.write_invocation_indexes(&self.invocation(id)?)?;
                    }
                    remove_if_present(&journal)
                })?;
                remaining -= 1;
            }
            if remaining == 0 {
                break;
            }
        }
        Ok(())
    }

    pub fn invocations(&self, offset: usize, limit: usize) -> RefineResult<Value> {
        self.repair_invocation_indexes(None)?;
        self.read_invocation_history(self.refine_dir.join("automation/history"), offset, limit)
    }

    pub fn goal_invocations(
        &self,
        goal_id: &str,
        offset: usize,
        limit: usize,
    ) -> RefineResult<Value> {
        if !valid_id(goal_id) {
            return Err(RefineError::InvalidInput("invalid Goal ID".into()));
        }
        self.repair_invocation_indexes(None)?;
        self.read_invocation_history(
            self.refine_dir
                .join("automation/goal-history")
                .join(goal_id),
            offset,
            limit,
        )
    }

    fn read_invocation_history(
        &self,
        directory: PathBuf,
        offset: usize,
        limit: usize,
    ) -> RefineResult<Value> {
        if !directory.exists() {
            return Ok(json!({"items": [], "offset": offset, "total": 0}));
        }
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .map_err(|e| RefineError::Io(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RefineError::Io(e.to_string()))?
            .into_iter()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort_by(|a, b| b.cmp(a));
        let total = paths.len();
        let items = paths
            .iter()
            .skip(offset)
            .take(limit.clamp(1, 100))
            .map(|p| read_json::<Value>(p))
            .collect::<RefineResult<Vec<_>>>()?;
        Ok(json!({"items": items, "offset": offset, "total": total}))
    }

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
                let contract = result_contract(&invocation, &pinned);
                let observational = pinned.skill.role == "plan"
                    || pinned.skill.role == "governance"
                    || invocation
                        .context
                        .data
                        .get("verification_only")
                        .and_then(Value::as_bool)
                        == Some(true);
                let git = crate::infrastructure::git::worktrees::FileGitWorktreeService::with_runtime_root(&invocation.context.cwd, runtime);
                let before = observational
                    .then(|| git.implementation_planning_observation())
                    .transpose()?;
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
                let provider = HostAgentProviderService::with_runtime_root(runtime);
                let mut last_error = String::new();
                for attempt in 0..=2 {
                    validate_authority()?;
                    let output = provider.invoke_detailed(ProviderInvocation { provider: invocation.context.provider.clone(), prompt: if attempt == 0 { prompt.clone() } else { format!("{prompt}\n\nThe previous response did not satisfy the result contract: {last_error}. Correct the response using retained evidence; do not repeat side effects.") }, session_id: None, cwd: Some(invocation.context.cwd.display().to_string()), stall_timeout_seconds: Some(seconds("agent_idle_timeout_seconds", 900)).filter(|v| *v > 0), process_metadata: metadata.clone() })?;
                    invocation.attempts.push(json!({"binding_id": pinned.binding.id, "attempt": attempt, "process_id": output.process_id, "raw_output": output.output, "workflow_revision":metadata.get("workflow_revision"), "operation_id":metadata.get("operation_id"), "diagnostic": null}));
                    self.save_invocation(&invocation)?;
                    validate_authority()?;
                    if let Some(before) = &before
                        && &git.implementation_planning_observation()? != before
                    {
                        return Err(RefineError::Conflict("observational Skill changed the checkout; changes and process evidence were retained".into()));
                    }
                    let parsed = <SkillResult as crate::application::agent_io::structured_output::Contract>::decode(&output.output)
                        .map_err(|e| RefineError::InvalidInput(e.to_string()))
                        .and_then(|result| { result.validate(id, &pinned.binding.id, &pinned.skill.role).map_err(RefineError::InvalidInput)?; validate_artifacts(&result)?; Ok(result) });
                    if let Some(last) = invocation.attempts.last_mut() {
                        last["diagnostic"] = json!(parsed.as_ref().err().map(ToString::to_string));
                    }
                    self.save_invocation(&invocation)?;
                    match parsed {
                        Ok(result) => return Ok(result),
                        Err(e) => last_error = e.to_string(),
                    }
                }
                Err(RefineError::Serialization(format!(
                    "Skill output contract failed after two repairs: {last_error}"
                )))
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
        .filter(|b| b.binding.mode == BindingMode::Blocking)
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

pub fn resolve_parameters(
    parameters: &[Parameter],
    explicit: &BTreeMap<String, Value>,
    mapped: &BTreeMap<String, Value>,
) -> RefineResult<BTreeMap<String, Value>> {
    let names: BTreeSet<_> = parameters.iter().map(|p| p.name.as_str()).collect();
    if let Some(name) = explicit.keys().find(|k| !names.contains(k.as_str())) {
        return Err(RefineError::InvalidInput(format!(
            "unknown parameter: {name}"
        )));
    }
    let mut values = BTreeMap::new();
    let mut missing = Vec::new();
    for p in parameters {
        match explicit
            .get(&p.name)
            .or_else(|| mapped.get(&p.name))
            .or(p.default.as_ref())
        {
            Some(value)
                if p.accepts(value)
                    && !(p.required && value.as_str().is_some_and(|v| v.trim().is_empty())) =>
            {
                values.insert(p.name.clone(), value.clone());
            }
            Some(_) => {
                return Err(RefineError::InvalidInput(format!(
                    "invalid parameter: {}",
                    p.name
                )));
            }
            None if p.required => missing.push(p.name.clone()),
            None => {}
        }
    }
    if !missing.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "missing required parameters: {}",
            missing.join(", ")
        )));
    }
    Ok(values)
}

fn field<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, key| {
        if v.is_array() {
            v.get(key.parse::<usize>().ok()?)
        } else {
            v.get(key)
        }
    })
}

fn result_contract(invocation: &EventInvocation, pinned: &PinnedBinding) -> Value {
    let artifacts = match pinned.skill.role.as_str() {
        "plan" => {
            json!({"plan": {"summary": "What changes and why", "checklist": [{"id": "P1", "description": "Implement and verify the requested behavior"}], "criticism_resolutions": []}})
        }
        "implement" => {
            json!({"implementation_evidence": {"checklist": [{"id": "copy the complete checklist ID", "outcome": "completed", "evidence": "Actual change and verification"}], "verification": ["command and observed result"]}})
        }
        "quality" => {
            json!({"tests": [{"test": "Observable requirement", "command": "non-interactive command whose exit 0 means pass", "status": "passed", "evidence": "Observed result"}]})
        }
        "governance" => {
            json!({"violations": [], "recovery_analysis": null, "recovery_round_prompt": null})
        }
        _ => json!({}),
    };
    json!({"invocation_id": invocation.id, "binding_id": pinned.binding.id, "role": pinned.skill.role, "outcome": "success", "summary": "What happened", "evidence": ["Observed supporting evidence"], "artifacts": artifacts})
}

impl crate::application::agent_io::structured_output::Contract for SkillResult {
    const LABEL: &'static str = "Skill completion result";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["skill_result", "result"];
    fn example() -> Self {
        Self {
            invocation_id: "invocation".into(),
            binding_id: "binding".into(),
            role: "task".into(),
            outcome: "success".into(),
            summary: "Task completed".into(),
            evidence: vec!["Observed evidence".into()],
            artifacts: json!({}),
        }
    }
}

fn validate_artifacts(result: &SkillResult) -> RefineResult<()> {
    use crate::application::agent_io::structured_output::Contract;
    use crate::model::goal::{ImplementationExecutionEvidence, ProposedImplementationPlan};
    if result.outcome != "success" {
        return Ok(());
    }
    match result.role.as_str() {
        "plan" => {
            ProposedImplementationPlan::decode(&result.artifacts["plan"].to_string())
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
        }
        "implement" => {
            ImplementationExecutionEvidence::decode(
                &result.artifacts["implementation_evidence"].to_string(),
            )
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        }
        "quality" => {
            for test in result.artifacts["tests"].as_array().into_iter().flatten() {
                for key in ["test", "command"] {
                    if test
                        .get(key)
                        .and_then(Value::as_str)
                        .is_none_or(|v| v.trim().is_empty())
                    {
                        return Err(RefineError::Serialization(format!(
                            "artifacts.tests requires a nonempty {key}"
                        )));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

impl EventInvocation {
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

fn remove_if_present(path: &std::path::Path) -> RefineResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RefineError::Io(e.to_string())),
    }
}
