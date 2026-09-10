use super::{EventInvocation, FileEventService, InvocationContext, InvocationState};
use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::work_items::FileWorkItemService;
use crate::application::workflow::WorkflowEngine;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::automation::{EventKind, valid_id};
use crate::model::workflow::GoalStatus;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
pub(crate) struct EventReservation {
    pub runtime: std::path::PathBuf,
    pub invocation_id: String,
    pub node: String,
    pub provider: String,
    pub target: String,
}
static ACTIVE: OnceLock<Mutex<BTreeMap<String, EventReservation>>> = OnceLock::new();
static DISPATCH: Mutex<()> = Mutex::new(());
pub(crate) fn reservations(runtime: &Path) -> Vec<EventReservation> {
    ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .filter(|v| v.runtime == runtime)
        .cloned()
        .collect()
}

impl FileEventService {
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

    pub fn manual_context(
        &self,
        target_root: &Path,
        body: &Value,
    ) -> RefineResult<InvocationContext> {
        let runtime = self.runtime()?;
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, runtime)
            .active_node_id()?;
        if body
            .get("node_id")
            .and_then(Value::as_str)
            .is_some_and(|n| !n.eq_ignore_ascii_case(&node))
        {
            return Err(RefineError::Conflict(
                "trigger this Event through its selected node's daemon".into(),
            ));
        }
        let settings = FileSettingsService::with_active_root(&self.refine_dir, runtime).load()?;
        let provider = settings
            .get("agent_cli")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("claude")
            .to_string();
        let goal_id = body
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut goal = Value::Null;
        let mut cwd = target_root.to_path_buf();
        let mut round_idx = None;
        let mut candidate_commit = None;
        if let Some(id) = &goal_id {
            goal = FileWorkItemService::new(&self.refine_dir).show_goal_detail(id)?;
            let owner = goal
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("default");
            if !owner.eq_ignore_ascii_case(&node) {
                return Err(RefineError::Conflict(format!(
                    "Goal {id} is owned by node {owner}"
                )));
            }
            round_idx = goal
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|r| r.len().checked_sub(1));
            candidate_commit = goal
                .get("candidate_commit")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(branch) = goal.get("branch_name").and_then(Value::as_str) {
                use crate::infrastructure::git::worktrees::FileGitWorktreeService;
                let git = FileGitWorktreeService::with_runtime_root(target_root, runtime);
                if let Some(worktree) = git.existing_worktree_for_branch(branch)? {
                    cwd = worktree;
                }
            }
        }
        Ok(InvocationContext {
            node_id: node.clone(),
            target_root: target_root.into(),
            cwd: cwd.clone(),
            provider,
            goal_id,
            round_idx,
            workflow_revision: None,
            candidate_commit,
            data: json!({"goal": super::execution::goal_context(&goal), "system": {"node_id": node, "project_root": target_root, "workspace": cwd}}),
            metadata: Default::default(),
        })
    }

    pub fn validate_manual_authority(&self, invocation: &EventInvocation) -> RefineResult<()> {
        if self.invocation(&invocation.id)?.state == InvocationState::Cancelled {
            return Err(RefineError::Conflict(
                "Event invocation was cancelled".into(),
            ));
        }
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, self.runtime()?)
            .active_node_id()?;
        if node != invocation.context.node_id {
            return Err(RefineError::Conflict("Event execution node changed".into()));
        }
        if let Some(id) = &invocation.context.goal_id {
            let goal = FileWorkItemService::new(&self.refine_dir).show_goal_detail(id)?;
            let pinned = invocation.context.data.get("goal").unwrap_or(&Value::Null);
            for key in ["status", "node_id", "candidate_commit", "event_generation"] {
                if invocation.context.data.get("occurrence").is_some()
                    && ["status", "event_generation"].contains(&key)
                {
                    continue;
                }
                if goal.get(key) != pinned.get(key) {
                    return Err(RefineError::Conflict(format!(
                        "Goal {id} {key} changed after this Event was requested"
                    )));
                }
            }
            let current_request = goal
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|r| r.last())
                .and_then(|r| r.get("prompt"));
            let pinned_request = pinned
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|r| r.last())
                .and_then(|r| r.get("prompt"));
            if current_request != pinned_request {
                return Err(RefineError::Conflict(
                    "Goal request changed after this Event was requested".into(),
                ));
            }
            if goal.get("rounds").and_then(Value::as_array).map(Vec::len)
                != pinned.get("rounds").and_then(Value::as_array).map(Vec::len)
            {
                return Err(RefineError::Conflict("Goal Round changed".into()));
            }
        }
        Ok(())
    }

    /// Called by the existing workflow worker; only pending records are inspected.
    pub fn dispatch_pending(&self, target_root: &Path) -> RefineResult<usize> {
        let _dispatch = DISPATCH.lock().unwrap_or_else(|e| e.into_inner());
        let runtime = self.runtime()?;
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, runtime)
            .active_node_id()?;
        self.repair_invocation_indexes(Some(&node))?;
        let directory = self.refine_dir.join("automation/pending").join(&node);
        if !directory.exists() {
            return Ok(0);
        }
        let engine = WorkflowEngine::with_target_root(runtime, target_root);
        engine.ensure_automation_running()?;
        let policy = engine.policy_for_refine_dir_and_node(&self.refine_dir, &node)?;
        let mut launched = 0;
        for entry in std::fs::read_dir(directory)
            .map_err(|e| RefineError::Io(e.to_string()))?
            .take(256)
        {
            let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
            if path.extension().and_then(|p| p.to_str()) != Some("json") {
                continue;
            }
            let record: Value = match read_json(&path) {
                Ok(record) => record,
                Err(_) if !path.exists() => continue,
                Err(e) => return Err(e),
            };
            if record.get("node_id").and_then(Value::as_str) != Some(&node) {
                continue;
            }
            let Some(id) = record.get("id").and_then(Value::as_str) else {
                continue;
            };
            let invocation = self.invocation(id)?;
            // Active workflow phases own these invocations and their capacity themselves.
            if (invocation.context.workflow_revision.is_some() && !invocation.state.terminal())
                || (invocation.state.terminal()
                    && !(invocation.state == InvocationState::Succeeded
                        && invocation.event.on_success.is_some()
                        && !invocation.action_applied))
            {
                continue;
            }
            if !engine.soft_capacity_available(
                &policy,
                &node,
                &invocation.context.provider,
                &target_root.display().to_string(),
            )? {
                break;
            }
            let key = format!("{}:{id}", self.refine_dir.display());
            let mut active = ACTIVE.get_or_init(Default::default).lock().unwrap();
            if active.len() >= 32 {
                break;
            }
            if active.contains_key(&key) {
                continue;
            }
            active.insert(
                key.clone(),
                EventReservation {
                    runtime: runtime.into(),
                    invocation_id: id.into(),
                    node: node.clone(),
                    provider: invocation.context.provider.clone(),
                    target: target_root.display().to_string(),
                },
            );
            drop(active);
            let service = self.clone();
            std::thread::spawn(move || {
                let result = service.execute(&invocation.id, || {
                    service.validate_manual_authority(&invocation)
                });
                match result {
                    Ok(mut result) if result.state == InvocationState::Succeeded => {
                        if let Err(error) = service.apply_success_action(&mut result) {
                            result.state = InvocationState::Error;
                            result.error = Some(error.to_string());
                            let _ = service.save_invocation(&result);
                        }
                    }
                    Ok(_) => {}
                    Err(RefineError::Degraded(message))
                        if message.starts_with("workspace is in use") => {}
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
                ACTIVE
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap()
                    .remove(&key);
            });
            launched += 1;
        }
        Ok(launched)
    }

    pub(super) fn apply_success_action(
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
            let applied = super::transitions::with_action(
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
            if !matches!(&applied, Err(RefineError::Conflict(message)) if message.starts_with(super::transitions::PENDING))
            {
                applied?;
            }
            invocation.action_applied = true;
            self.save_invocation(invocation)
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

    pub(super) fn terminate_invocation_processes(&self, id: &str) -> RefineResult<()> {
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
