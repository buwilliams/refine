use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessSupervisor, write_json_atomically,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::chat::FileChatService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_registry::FileProjectRegistryService;
use crate::tools::product::work_items::{
    BulkGoalSelection, BulkSkippedDetail, BulkUpdateResult, FileWorkItemService,
    GoalCancellationExpectation,
};

const DEFAULT_AGENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Node-local process control. Goal state decides workflow meaning; process records only identify
/// observable local work that can be stopped.
#[derive(Clone, Debug)]
pub struct FileProcessControlService {
    runtime_root: PathBuf,
    refine_dir: Option<PathBuf>,
    agent_exit_timeout: Duration,
}

impl FileProcessControlService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: None,
            agent_exit_timeout: DEFAULT_AGENT_EXIT_TIMEOUT,
        }
    }

    pub fn with_refine_dir(
        runtime_root: impl Into<PathBuf>,
        refine_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: Some(refine_dir.into()),
            agent_exit_timeout: DEFAULT_AGENT_EXIT_TIMEOUT,
        }
    }

    pub fn stop(&self, process_id: &str, signal: &str) -> RefineResult<Value> {
        validate_process_id(process_id)?;
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        if let Some((supervisor, process)) = self.find_managed_process(process_id)? {
            return self.stop_managed_process(supervisor, process, signal);
        }
        if let Some(session_id) = process_id.strip_prefix("chat-session-") {
            let refine_dir = self.resolve_refine_dir()?;
            let chat = FileChatService::with_runtime_root(&refine_dir, &self.runtime_root);
            let existing = chat
                .list_sessions()?
                .into_iter()
                .find(|session| session.id == session_id)
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Chat session {session_id} was not found"))
                })?;
            let goal_id = match &existing.attachment {
                crate::tools::product::chat::ChatAttachment::Goal(goal_id) => Some(goal_id.clone()),
                _ => None,
            };
            let expectation = goal_id
                .as_deref()
                .map(|goal_id| self.goal_expectation(goal_id))
                .transpose()?;
            let session = chat.stop(session_id)?;
            let goal = match (goal_id.as_deref(), expectation.as_ref()) {
                (Some(goal_id), Some(expectation)) => Some(
                    FileWorkItemService::new(&refine_dir)
                        .fail_goal_after_process_stop_if_current(goal_id, expectation)?,
                ),
                _ => None,
            };
            let goal_failed = goal
                .as_ref()
                .is_some_and(|goal| goal.goal.status == GoalStatus::Failed);
            return Ok(json!({
                "stopped": true,
                "process": {
                    "id": process_id,
                    "kind": "chat",
                    "session_id": session.id,
                    "status": "stopped"
                },
                "termination": {
                    "confirmed_exit": true,
                    "already_idle": true
                },
                "goal": goal.map(|goal| goal.goal),
                "goal_failed": goal_failed,
                "goal_requeued": false,
                "worktrees_retained": true
            }));
        }
        Err(RefineError::NotFound(format!(
            "Process {process_id} was not found"
        )))
    }

    fn stop_managed_process(
        &self,
        supervisor: FileProcessSupervisor,
        process: ManagedProcess,
        signal: &str,
    ) -> RefineResult<Value> {
        let metadata = process_metadata(&process);
        let goal_id = metadata
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let session_id = metadata
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let chat_owned = metadata.get("kind").and_then(Value::as_str) == Some("chat");
        let expectation = match goal_id.as_deref() {
            Some(goal_id) => Some(self.goal_expectation(goal_id)?),
            None => None,
        };
        let mut termination = self.terminate_with_escalation(&supervisor, &process, signal)?;
        termination = supervisor
            .cleanup_confirmed_exit(&process, termination)
            .map_err(|failure| {
                let _confirmed_exit = failure.outcome;
                failure.error
            })?;
        if chat_owned && let Some(session_id) = session_id {
            let refine_dir = self.resolve_refine_dir()?;
            FileChatService::with_runtime_root(&refine_dir, &self.runtime_root)
                .stop(&session_id)?;
        }
        let goal = match (goal_id.as_deref(), expectation.as_ref()) {
            (Some(goal_id), Some(expectation)) => {
                let work_items = FileWorkItemService::new(self.resolve_refine_dir()?);
                Some(work_items.fail_goal_after_process_stop_if_current(goal_id, expectation)?)
            }
            _ => None,
        };
        let receipt = json!({
            "schema_version": 1,
            "process_id": process.id,
            "goal_id": goal_id,
            "termination": termination,
            "goal_status": goal.as_ref().map(|goal| goal.goal.status.as_str()),
            "goal_failed": goal.as_ref().is_some_and(|goal| goal.goal.status == GoalStatus::Failed),
            "goal_requeued": false,
            "worktrees_retained": true,
            "recorded_at": Utc::now().to_rfc3339()
        });
        self.write_stop_receipt(&process.id, &receipt)?;
        let mut process_json = process.api_json();
        if let Some(object) = process_json.as_object_mut() {
            object.insert("status".to_string(), json!("stopped"));
            object.insert("state".to_string(), json!("stopped"));
        }
        Ok(json!({
            "stopped": true,
            "process": process_json,
            "termination": receipt["termination"],
            "goal": goal.map(|goal| goal.goal),
            "goal_failed": receipt["goal_failed"],
            "goal_requeued": receipt["goal_requeued"],
            "worktrees_retained": true
        }))
    }

    /// Commits terminal Goal cancellation first, then best-effort stops all matching local
    /// workers. A stop failure is returned as evidence and never rolls the Goal back.
    pub fn cancel_goal(&self, goal_id: &str) -> RefineResult<Value> {
        let goal_id = goal_id.trim();
        if goal_id.is_empty() {
            return Err(RefineError::InvalidInput("Goal id is required".to_string()));
        }
        let refine_dir = self.resolve_refine_dir()?;
        let work_items = FileWorkItemService::new(&refine_dir);
        let goal = work_items.cancel_goal_summary(goal_id)?;
        let mut stopped = Vec::new();
        let mut failures = Vec::new();
        for (supervisor, process) in self.managed_processes_for_goal(goal_id)? {
            match self
                .terminate_with_escalation(&supervisor, &process, "terminate")
                .and_then(|outcome| {
                    supervisor
                        .cleanup_confirmed_exit(&process, outcome)
                        .map_err(|failure| {
                            let _confirmed_exit = failure.outcome;
                            failure.error
                        })
                }) {
                Ok(outcome) => stopped.push(json!(outcome)),
                Err(error) => failures.push(json!({
                    "process_id": process.id,
                    "error": error.to_string()
                })),
            }
        }
        let durable = work_items.show_goal_summary(goal_id)?;
        Ok(json!({
            "cancelled": durable.goal.status == GoalStatus::Cancelled,
            "goal_id": goal_id,
            "goal": goal.goal,
            "processes": stopped,
            "process_failures": failures,
            "worktrees_retained": true
        }))
    }

    fn terminate_with_escalation(
        &self,
        supervisor: &FileProcessSupervisor,
        process: &ManagedProcess,
        signal: &str,
    ) -> RefineResult<crate::process::subprocess::ConfirmedProcessExit> {
        match supervisor.terminate_owned_and_confirm_exit(process, signal, self.agent_exit_timeout)
        {
            Ok(outcome) => Ok(outcome),
            Err(RefineError::Degraded(_)) if signal != "kill" => supervisor
                .terminate_owned_and_confirm_exit(process, "kill", self.agent_exit_timeout),
            Err(error) => Err(error),
        }
    }

    pub fn bulk_cancel_goals(
        &self,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkUpdateResult> {
        let refine_dir = self.resolve_refine_dir()?;
        let work_items = FileWorkItemService::new(&refine_dir);
        let ids = if let Some(selected_ids) = &selection.selected_ids {
            let excluded = selection
                .exclude_ids
                .iter()
                .map(|id| id.trim().to_uppercase())
                .collect::<BTreeSet<_>>();
            selected_ids
                .iter()
                .map(|id| id.trim().to_uppercase())
                .filter(|id| !id.is_empty() && !excluded.contains(id))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            work_items.select_bulk_goal_ids(&selection)?
        };
        let active_node = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let mut updated_ids = Vec::new();
        let mut skipped_details = Vec::new();
        let mut failures = Vec::new();
        for goal_id in ids {
            let selected = match work_items.show_goal_summary(&goal_id) {
                Ok(goal) => goal,
                Err(error) => {
                    failures.push(bulk_goal_failure(&goal_id, &error));
                    continue;
                }
            };
            if selected.goal.status == GoalStatus::Done {
                skipped_details.push(BulkSkippedDetail {
                    id: goal_id,
                    reason: "status:done".to_string(),
                });
                continue;
            }
            let owner = selected.goal.node_id.as_deref().unwrap_or("default");
            if owner != active_node {
                skipped_details.push(BulkSkippedDetail {
                    id: goal_id,
                    reason: format!("node:{owner}"),
                });
                continue;
            }
            match self.cancel_goal(&goal_id) {
                Ok(result) if result.get("cancelled").and_then(Value::as_bool) == Some(true) => {
                    updated_ids.push(goal_id)
                }
                Ok(result) => failures.push(json!({
                    "id": goal_id,
                    "error": {"code": "partial", "evidence": result}
                })),
                Err(error) => failures.push(bulk_goal_failure(&goal_id, &error)),
            }
        }
        Ok(BulkUpdateResult {
            updated: updated_ids.len(),
            ids: updated_ids,
            field: "status".to_string(),
            value: GoalStatus::Cancelled.as_str().to_string(),
            skipped: skipped_details.len(),
            skipped_details,
            failed: failures.len(),
            failures,
        })
    }

    fn goal_expectation(&self, goal_id: &str) -> RefineResult<GoalCancellationExpectation> {
        let work_items = FileWorkItemService::new(self.resolve_refine_dir()?);
        let goal = work_items.show_goal_summary(goal_id)?;
        if goal.goal.status == GoalStatus::Done {
            return Err(RefineError::InvalidInput(format!(
                "done Goal {goal_id} cannot be failed by process Stop"
            )));
        }
        Ok(GoalCancellationExpectation {
            status: goal.goal.status,
            round_count: goal.goal.round_count,
            updated: goal.goal.updated,
            node_id: goal
                .goal
                .node_id
                .filter(|node| !node.is_empty())
                .unwrap_or_else(|| "default".to_string()),
        })
    }

    fn find_managed_process(
        &self,
        process_id: &str,
    ) -> RefineResult<Option<(FileProcessSupervisor, ManagedProcess)>> {
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            match supervisor.inspect(process_id) {
                Ok(process) => return Ok(Some((supervisor, process))),
                Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    fn managed_processes_for_goal(
        &self,
        goal_id: &str,
    ) -> RefineResult<Vec<(FileProcessSupervisor, ManagedProcess)>> {
        let mut matches = Vec::new();
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                let metadata = process_metadata(&process);
                if metadata.get("goal_id").and_then(Value::as_str) == Some(goal_id)
                    && process_may_be_stopped_by_goal_cancellation(&metadata)
                    && FileProcessSupervisor::process_is_alive(&process)?
                {
                    matches.push((supervisor.clone(), process));
                }
            }
        }
        matches.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        Ok(matches)
    }

    fn resolve_refine_dir(&self) -> RefineResult<PathBuf> {
        if let Some(refine_dir) = &self.refine_dir {
            return Ok(refine_dir.clone());
        }
        let registry = FileProjectRegistryService::new(&self.runtime_root, None).load()?;
        let target_root = registry
            .active_app
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| RefineError::Degraded("runtime has no active app".to_string()))?;
        refine_dir_for_target_root(Path::new(&target_root))
    }

    fn write_stop_receipt(&self, process_id: &str, receipt: &Value) -> RefineResult<()> {
        let directory = self.runtime_root.join("process-stop-outcomes");
        fs::create_dir_all(&directory).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process-stop outcome directory {}: {error}",
                directory.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(receipt).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process-stop outcome: {error}"))
        })?;
        write_json_atomically(
            &directory.join(format!("process-{process_id}-{}.json", Uuid::new_v4())),
            &encoded,
            "process-stop outcome",
        )
    }
}

fn managed_process_roots(runtime_root: &Path) -> [PathBuf; 2] {
    [runtime_root.to_path_buf(), runtime_root.join("agents")]
}

fn process_metadata(process: &ManagedProcess) -> Map<String, Value> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| details.as_object().cloned())
        .unwrap_or_default()
}

fn process_may_be_stopped_by_goal_cancellation(metadata: &Map<String, Value>) -> bool {
    metadata
        .get("side_effect_committed")
        .and_then(Value::as_bool)
        != Some(true)
}

fn validate_process_id(process_id: &str) -> RefineResult<()> {
    if process_id.trim().is_empty() || process_id.contains('/') || process_id.contains('\\') {
        return Err(RefineError::InvalidInput(
            "process id is required and cannot contain path separators".to_string(),
        ));
    }
    Ok(())
}

fn bulk_goal_failure(goal_id: &str, error: &RefineError) -> Value {
    json!({
        "id": goal_id,
        "error": {
            "code": "goal_cancellation_failed",
            "message": error.to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_cancellation_skips_work_past_a_side_effect_boundary() {
        let ordinary = Map::from_iter([("kind".to_string(), json!("workflow"))]);
        assert!(process_may_be_stopped_by_goal_cancellation(&ordinary));

        let committed = Map::from_iter([
            ("kind".to_string(), json!("workflow")),
            ("side_effect_committed".to_string(), json!(true)),
        ]);
        assert!(!process_may_be_stopped_by_goal_cancellation(&committed));
    }
}
