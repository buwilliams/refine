use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessPauseState, ProcessSupervisor,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::tools::product::chat::{ChatAttachment, ChatSessionRecord, FileChatService};
use crate::tools::product::project_state::RuntimeProjection;

#[derive(Clone, Debug)]
pub struct FileProcessStatusService {
    pub runtime_root: PathBuf,
    pub refine_dir: Option<PathBuf>,
}

impl FileProcessStatusService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: None,
        }
    }

    pub fn with_refine_dir(
        runtime_root: impl Into<PathBuf>,
        refine_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: Some(refine_dir.into()),
        }
    }

    pub fn summary(&self) -> RefineResult<Value> {
        process_summary_value_with_chat_sessions(&self.runtime_root, self.refine_dir.as_deref())
    }

    pub fn stream(&self, process_id: &str) -> RefineResult<String> {
        self.resolve(process_id, |supervisor| supervisor.stream(process_id))
    }

    pub fn stop(&self, process_id: &str, signal: &str) -> RefineResult<ManagedProcess> {
        self.resolve(process_id, |supervisor| {
            supervisor.signal(process_id, signal)
        })
    }

    fn resolve<T>(
        &self,
        process_id: &str,
        operation: impl Fn(&FileProcessSupervisor) -> RefineResult<T>,
    ) -> RefineResult<T> {
        validate_process_id(process_id)?;
        for process_root in managed_process_roots(&self.runtime_root) {
            match operation(&FileProcessSupervisor::new(process_root)) {
                Ok(value) => return Ok(value),
                Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Err(RefineError::NotFound(format!(
            "Process {process_id} was not found"
        )))
    }
}

pub fn process_summary_value(runtime_root: &Path) -> RefineResult<Value> {
    process_summary_value_with_chat_sessions(runtime_root, None)
}

pub fn process_summary_value_with_chat_sessions(
    runtime_root: &Path,
    refine_dir: Option<&Path>,
) -> RefineResult<Value> {
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let pause_state = supervisor.pause_state()?;
    let mut process_values = Vec::new();
    let mut seen_process_ids = BTreeSet::new();
    for process_root in managed_process_roots(runtime_root) {
        for process in FileProcessSupervisor::new(process_root).recover()? {
            if !seen_process_ids.insert(process.id.clone()) {
                continue;
            }
            let mut value = process.api_json();
            apply_process_management_actions(&mut value, &pause_state);
            if is_current_process_api_value(&value) {
                process_values.push(value);
            }
        }
    }
    append_chat_session_processes(&mut process_values, runtime_root, refine_dir)?;
    let runner_reachable = required_runner_workers_reachable(&process_values);
    Ok(json!({
        "runner_reachable": runner_reachable,
        "paused": pause_state.background_processes_stopped || pause_state.agents_paused,
        "background_processes_stopped": pause_state.background_processes_stopped,
        "agents_paused": pause_state.agents_paused,
        "processes": process_values,
        "runner_work": runner_work_summary(runtime_root, pause_state.background_processes_stopped),
        "backend": {
            "process_model": "supervisor"
        }
    }))
}

fn managed_process_roots(runtime_root: &Path) -> [PathBuf; 2] {
    [runtime_root.to_path_buf(), runtime_root.join("agents")]
}

pub fn runtime_process_summary_value(runtime: &RuntimeProjection) -> Value {
    let mut summary = runtime.supervisor.clone().unwrap_or_default();
    let pause_state = process_pause_state_from_summary(&summary);
    summary.insert(
        "processes".to_string(),
        Value::Array(
            runtime
                .processes
                .iter()
                .filter(|process| is_current_process_object(process))
                .cloned()
                .map(|process| {
                    let mut value = Value::Object(process);
                    apply_process_management_actions(&mut value, &pause_state);
                    value
                })
                .collect(),
        ),
    );
    summary
        .entry("runner_reachable".to_string())
        .or_insert(Value::Bool(false));
    summary
        .entry("runner_work".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    Value::Object(summary)
}

pub fn runtime_process_status_value(runtime: &RuntimeProjection) -> Value {
    process_status_value(&runtime_process_summary_value(runtime))
}

pub fn process_status_value(summary: &Value) -> Value {
    let mut summary = summary.as_object().cloned().unwrap_or_default();
    let current_processes = summary
        .get("processes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|process| is_current_process_api_value(process))
        .collect::<Vec<_>>();
    let process_count = current_processes.len();
    let agent_count = current_processes
        .iter()
        .filter(|process| process.get("kind").and_then(Value::as_str) == Some("agent"))
        .count();
    let running_count = current_processes
        .iter()
        .filter(|process| process.get("status").and_then(Value::as_str) == Some("running"))
        .count();
    summary.insert("process_count".to_string(), json!(process_count));
    summary.insert("agent_count".to_string(), json!(agent_count));
    summary.insert("running_process_count".to_string(), json!(running_count));
    summary.insert("processes".to_string(), Value::Array(Vec::new()));
    summary
        .entry("runner_reachable".to_string())
        .or_insert(Value::Bool(false));
    summary
        .entry("runner_work".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    Value::Object(summary)
}

pub fn is_current_process_object(process: &JsonObject) -> bool {
    let status = process.get("status").and_then(Value::as_str).unwrap_or("");
    !is_terminal_process_status(status)
}

fn append_chat_session_processes(
    process_values: &mut Vec<Value>,
    runtime_root: &Path,
    refine_dir: Option<&Path>,
) -> RefineResult<()> {
    let Some(refine_dir) = refine_dir else {
        return Ok(());
    };
    let existing_session_ids = process_values
        .iter()
        .filter_map(|process| process.get("session_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let service = FileChatService::with_runtime_root(refine_dir, runtime_root);
    for session in service.list_sessions()? {
        if !is_process_visible_chat_session(&session) || existing_session_ids.contains(&session.id)
        {
            continue;
        }
        process_values.push(chat_session_process_value(&session));
    }
    Ok(())
}

fn is_process_visible_chat_session(session: &ChatSessionRecord) -> bool {
    !session.closed
        && matches!(session.mode.as_str(), "standalone" | "goal" | "supervisor")
        && matches!(
            session.attachment,
            ChatAttachment::Standalone | ChatAttachment::Goal(_) | ChatAttachment::Supervisor
        )
}

fn chat_session_process_value(session: &ChatSessionRecord) -> Value {
    let goal_id = match &session.attachment {
        ChatAttachment::Goal(goal_id) => Some(goal_id.as_str()),
        _ => None,
    };
    let details = [
        Some(session.provider.as_str()),
        Some(session.mode.as_str()),
        goal_id,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    json!({
        "id": format!("chat-session-{}", session.id),
        "kind": "chat",
        "label": if session.mode == "goal" { "Goal agent session" } else { "Standalone chat" },
        "status": if session.in_flight || session.queue_dispatching { "running" } else { "idle" },
        "pid": null,
        "details": details,
        "session_id": &session.id,
        "mode": &session.mode,
        "provider": &session.provider,
        "goal_id": goal_id,
        "started_at": &session.created_at,
        "updated_at": &session.updated_at,
        "output_available": false,
        "cpu_priority": {"label": "-"},
        "max_memory": {"label": "-"},
        "isolation": "in_process",
        "management_actions": ["stop_chat"],
        "actions": ["stop"]
    })
}

fn apply_process_management_actions(value: &mut Value, pause_state: &ProcessPauseState) {
    let actions = process_management_actions(value, pause_state);
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if actions.is_empty() {
        object.remove("management_actions");
    } else {
        object.insert("management_actions".to_string(), json!(actions));
    }
}

fn process_management_actions(value: &Value, pause_state: &ProcessPauseState) -> Vec<&'static str> {
    let Some(process) = value.as_object() else {
        return Vec::new();
    };
    let kind = process.get("kind").and_then(Value::as_str).unwrap_or("");
    let workflow_toggle = if pause_state.background_processes_stopped || pause_state.agents_paused {
        "unpause_workflow"
    } else {
        "pause_workflow"
    };
    match kind {
        "daemon" | "supervisor" => vec![workflow_toggle],
        "workflow_automation" | "agent_automation" | "background_processes" => {
            vec![workflow_toggle, "hard_reset_worktree"]
        }
        "agent" if process.get("goal_id").and_then(Value::as_str).is_some() => {
            vec!["cancel_agent"]
        }
        "chat" if process.get("session_id").and_then(Value::as_str).is_some() => {
            vec!["stop_chat"]
        }
        _ => Vec::new(),
    }
}

fn process_pause_state_from_summary(summary: &JsonObject) -> ProcessPauseState {
    ProcessPauseState {
        background_processes_stopped: summary
            .get("background_processes_stopped")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        agents_paused: summary
            .get("agents_paused")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn is_current_process_api_value(process: &Value) -> bool {
    let Some(process) = process.as_object() else {
        return false;
    };
    is_current_process_object(process)
}

fn is_terminal_process_status(status: &str) -> bool {
    matches!(
        status,
        "exited" | "failed" | "stopped" | "cancelled" | "complete" | "completed" | "interrupted"
    )
}

fn required_runner_workers_reachable(processes: &[Value]) -> bool {
    ["workflow", "git-sync"].into_iter().all(|required| {
        processes.iter().any(|process| {
            process.get("kind").and_then(Value::as_str) == Some("runner")
                && process.get("worker_kind").and_then(Value::as_str) == Some(required)
                && process.get("status").and_then(Value::as_str) == Some("running")
        })
    })
}

fn runner_work_summary(runtime_root: &Path, background_stopped: bool) -> Value {
    let status = if background_stopped { "paused" } else { "idle" };
    let operations = FileOperationRegistry::new(runtime_root)
        .recover()
        .unwrap_or_default();
    let merger_operation = operations
        .iter()
        .rev()
        .find(|operation| operation.owner.starts_with("merger:"));
    let plan_extract_operation = operations
        .iter()
        .rev()
        .find(|operation| operation.owner == "import:extract:plan");
    let merger_status = if background_stopped {
        "paused".to_string()
    } else {
        merger_operation
            .map(|operation| operation.state.as_api_status().to_string())
            .unwrap_or_else(|| "idle".to_string())
    };
    let merger_goal_id = merger_operation
        .and_then(|operation| operation.owner.strip_prefix("merger:"))
        .map(ToString::to_string);
    let plan_extract_status = if background_stopped {
        "paused".to_string()
    } else {
        plan_extract_operation
            .map(|operation| operation.state.as_api_status().to_string())
            .unwrap_or_else(|| "idle".to_string())
    };
    let plan_extract_details = plan_extract_operation
        .and_then(|operation| operation.progress.get("message").and_then(Value::as_str))
        .unwrap_or("Plan Draft extraction is ready for Draft Feature requests");
    let mut rows = [
        ("merger", "serial Goal branch merger"),
        (
            "plan_draft_extractor",
            "Plan Draft extraction is ready for Draft Feature requests",
        ),
        (
            "target_app_builder",
            "target-app build worker is ready for manual build requests",
        ),
        (
            "target_app_config_generator",
            "target-app config generation is ready for Smoke AI-backed requests",
        ),
        (
            "sqlite_cache_rebuild",
            "projection cache rebuild worker is ready for manual rebuild requests",
        ),
        (
            "activity_log_cleanup",
            "activity log cleanup worker is ready for retention cleanup requests",
        ),
    ]
    .into_iter()
    .map(|(kind, details)| {
        if kind == "merger" {
            json!({
                "kind": kind,
                "status": merger_status,
                "elapsed_seconds": 0,
                "queued": 0,
                "details": details,
                "operation_id": merger_operation.map(|operation| operation.id.clone()),
                "goal_id": merger_goal_id
            })
        } else if kind == "plan_draft_extractor" {
            json!({
                "kind": kind,
                "status": plan_extract_status,
                "elapsed_seconds": 0,
                "queued": 0,
                "details": plan_extract_details,
                "operation_id": plan_extract_operation.map(|operation| operation.id.clone())
            })
        } else {
            json!({
                "kind": kind,
                "status": status,
                "elapsed_seconds": 0,
                "queued": 0,
                "details": details
            })
        }
    })
    .collect::<Vec<_>>();
    Value::Array(std::mem::take(&mut rows))
}

fn validate_process_id(process_id: &str) -> RefineResult<()> {
    if process_id.trim().is_empty() || process_id.contains(['/', '\\']) {
        return Err(RefineError::InvalidInput(
            "process id is required".to_string(),
        ));
    }
    Ok(())
}
