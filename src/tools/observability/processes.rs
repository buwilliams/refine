use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::tools::product::chat::{ChatAttachment, ChatSessionRecord, FileChatService};
use crate::tools::product::project_state::RuntimeProjection;

const LEGACY_PROCESS_ROOT_ENTRY_LIMIT: usize = 512;

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

    pub fn stop(&self, process_id: &str, signal: &str) -> RefineResult<ManagedProcess> {
        validate_process_id(process_id)?;
        FileProcessSupervisor::new(&self.runtime_root).signal(process_id, signal)
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
    for process_root in process_roots(runtime_root, refine_dir) {
        let supervisor = FileProcessSupervisor::new(&process_root);
        let processes = if process_root == runtime_root {
            supervisor.recover()?
        } else {
            supervisor.recover_owner(ProcessOwner::TargetApp)?
        };
        for process in processes {
            if !seen_process_ids.insert(process.id.clone()) {
                continue;
            }
            let value = process.api_json();
            if is_current_process_api_value(&value) {
                process_values.push(value);
            }
        }
    }
    append_chat_session_processes(&mut process_values, runtime_root, refine_dir)?;
    let runner_reachable = runner_reachable_value(runtime_root);
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

pub fn runtime_process_summary_value(runtime: &RuntimeProjection) -> Value {
    let mut summary = runtime
        .supervisor
        .clone()
        .unwrap_or_else(serde_json::Map::new);
    summary.insert(
        "processes".to_string(),
        Value::Array(
            runtime
                .processes
                .iter()
                .filter(|process| is_current_process_object(process))
                .cloned()
                .map(Value::Object)
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
    let mut summary = runtime
        .supervisor
        .clone()
        .unwrap_or_else(serde_json::Map::new);
    let current_processes = runtime
        .processes
        .iter()
        .filter(|process| is_current_process_object(process))
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

fn process_roots(runtime_root: &Path, refine_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![runtime_root.to_path_buf()];
    if let Some(refine_dir) = refine_dir {
        let project_runtime_root = refine_dir.join("runtime");
        if project_runtime_root != runtime_root
            && should_scan_legacy_process_root(&project_runtime_root)
        {
            roots.push(project_runtime_root);
        }
    }
    roots
}

fn should_scan_legacy_process_root(project_runtime_root: &Path) -> bool {
    let processes_dir = project_runtime_root.join("processes");
    if !processes_dir.exists() {
        return true;
    }
    fs::read_dir(&processes_dir)
        .map(|entries| {
            entries.take(LEGACY_PROCESS_ROOT_ENTRY_LIMIT + 1).count()
                <= LEGACY_PROCESS_ROOT_ENTRY_LIMIT
        })
        .unwrap_or(false)
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
        && matches!(session.mode.as_str(), "standalone" | "gap")
        && matches!(
            session.attachment,
            ChatAttachment::Standalone | ChatAttachment::Gap(_)
        )
}

fn chat_session_process_value(session: &ChatSessionRecord) -> Value {
    let gap_id = match &session.attachment {
        ChatAttachment::Gap(gap_id) => Some(gap_id.as_str()),
        _ => None,
    };
    let details = [
        Some(session.provider.as_str()),
        Some(session.mode.as_str()),
        gap_id,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    json!({
        "id": format!("chat-session-{}", session.id),
        "kind": "chat",
        "label": if session.mode == "gap" { "Gap chat" } else { "Standalone chat" },
        "status": if session.in_flight || session.queue_dispatching { "running" } else { "idle" },
        "pid": null,
        "details": details,
        "session_id": &session.id,
        "mode": &session.mode,
        "provider": &session.provider,
        "gap_id": gap_id,
        "started_at": &session.created_at,
        "updated_at": &session.updated_at,
        "output_available": false,
        "cpu_priority": {"label": "-"},
        "max_memory": {"label": "-"},
        "isolation": "in_process",
        "actions": ["stop"]
    })
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

fn runner_reachable_value(runtime_root: &Path) -> bool {
    let path = runtime_root.join("runner-health.json");
    let Ok(bytes) = fs::read(&path) else {
        return true;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return true;
    };
    value
        .get("runner_reachable")
        .or_else(|| value.get("reachable"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
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
    let merger_gap_id = merger_operation
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
        ("merger", "serial Gap branch merger"),
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
                "gap_id": merger_gap_id
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
    if process_id.trim().is_empty() || process_id.contains('/') {
        return Err(RefineError::InvalidInput(
            "process id is required".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_roots_skip_oversized_legacy_runtime_process_registry() {
        let temp_root = std::env::temp_dir().join(format!(
            "refine-legacy-process-roots-{}",
            uuid::Uuid::new_v4()
        ));
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join("app/.refine");
        let legacy_processes = refine_dir.join("runtime/processes");
        fs::create_dir_all(&runtime_root).unwrap();
        fs::create_dir_all(&legacy_processes).unwrap();
        for index in 0..=LEGACY_PROCESS_ROOT_ENTRY_LIMIT {
            fs::write(legacy_processes.join(format!("proc-{index}.json")), "{}").unwrap();
        }

        assert_eq!(
            process_roots(&runtime_root, Some(&refine_dir)),
            vec![runtime_root]
        );

        fs::remove_dir_all(temp_root).unwrap();
    }
}
