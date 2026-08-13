use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::process::runner::BACKGROUND_RUNNERS;
use crate::process::subprocess::{FileProcessSupervisor, ProcessPauseState, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::tools::host::project_layout::git_common_dir;
use crate::tools::product::chat::{ChatAttachment, ChatSessionRecord, FileChatService};
use crate::tools::product::project_projection::RuntimeProjection;

const REPOSITORY_DISK_USAGE_CACHE_TTL: Duration = Duration::from_secs(15);

static REPOSITORY_DISK_USAGE_CACHE: OnceLock<Mutex<BTreeMap<PathBuf, (Instant, u64)>>> =
    OnceLock::new();

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
        let mut summary = process_summary_value_with_chat_sessions(
            &self.runtime_root,
            self.refine_dir.as_deref(),
        )?;
        enrich_process_resource_usage(&mut summary);
        Ok(summary)
    }

    pub fn stream(&self, process_id: &str) -> RefineResult<String> {
        self.resolve(process_id, |supervisor| supervisor.stream(process_id))
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
            if !process.is_runtime_projection_visible() {
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
    let background_workers = background_worker_values(&process_values, &pause_state);
    Ok(json!({
        "runner_reachable": runner_reachable,
        "paused": pause_state.workflow_paused,
        "workflow_paused": pause_state.workflow_paused,
        "disabled_background_workers": pause_state.disabled_background_workers,
        // Wire-compatible aliases. Both are derived from the single workflow gate.
        "background_processes_stopped": pause_state.workflow_paused,
        "agents_paused": pause_state.workflow_paused,
        "processes": process_values,
        "background_workers": background_workers,
        "runner_work": runner_work_summary(runtime_root),
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
    apply_pause_state_aliases(&mut summary, &pause_state);
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

pub fn enrich_process_resource_usage(summary: &mut Value) {
    let Some(summary) = summary.as_object_mut() else {
        return;
    };
    let mut pids = BTreeSet::new();
    for key in ["processes", "background_workers"] {
        for process in summary
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(pid) = process.get("pid").and_then(Value::as_u64) {
                pids.insert(pid as u32);
            }
        }
    }
    let observations = process_resource_observations(&pids);
    for key in ["processes", "background_workers"] {
        let Some(processes) = summary.get_mut(key).and_then(Value::as_array_mut) else {
            continue;
        };
        for process in processes {
            let Some(object) = process.as_object_mut() else {
                continue;
            };
            let Some(pid) = object.get("pid").and_then(Value::as_u64) else {
                continue;
            };
            let Some((memory_used_bytes, processor_used_percent)) = observations.get(&(pid as u32))
            else {
                continue;
            };
            object.insert("memory_used_bytes".to_string(), json!(memory_used_bytes));
            object.insert(
                "processor_used_percent".to_string(),
                json!(processor_used_percent),
            );
        }
    }
}

fn process_resource_observations(pids: &BTreeSet<u32>) -> BTreeMap<u32, (u64, f64)> {
    if pids.is_empty() {
        return BTreeMap::new();
    }
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = Command::new("ps")
        .args(["-o", "pid=,rss=,pcpu=", "-p", &pid_list])
        .output()
    else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let rss_kib = fields.next()?.parse::<u64>().ok()?;
            let processor_percent = fields.next()?.replace(',', ".").parse::<f64>().ok()?;
            Some((pid, (rss_kib.saturating_mul(1024), processor_percent)))
        })
        .collect()
}

pub fn repository_disk_usage_value(repository_root: &Path) -> Value {
    let common_dir = git_common_dir(repository_root).ok();
    let mut measured_roots = vec![repository_root.to_path_buf()];
    if let Some(common_dir) = common_dir.as_ref()
        && !common_dir.starts_with(repository_root)
    {
        measured_roots.push(common_dir.clone());
    }
    let bytes = measured_roots
        .iter()
        .filter_map(|path| cached_directory_disk_usage_bytes(path))
        .fold(0_u64, u64::saturating_add);
    json!({
        "bytes": bytes,
        "repository_root": repository_root,
        "git_common_dir": common_dir,
        "includes_git_worktrees": common_dir.is_some()
    })
}

fn cached_directory_disk_usage_bytes(path: &Path) -> Option<u64> {
    let cache = REPOSITORY_DISK_USAGE_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some((measured_at, bytes)) = cache.get(path)
        && measured_at.elapsed() < REPOSITORY_DISK_USAGE_CACHE_TTL
    {
        return Some(*bytes);
    }
    let output = Command::new("du").args(["-sk"]).arg(path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let kib = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    let bytes = kib.saturating_mul(1024);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(path.to_path_buf(), (Instant::now(), bytes));
    }
    Some(bytes)
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
        && matches!(session.mode.as_str(), "standalone" | "goal")
        && matches!(
            session.attachment,
            ChatAttachment::Standalone | ChatAttachment::Goal(_)
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
        "management_actions": ["stop_agent"],
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
    let workflow_toggle = if pause_state.workflow_paused {
        "unpause_workflow"
    } else {
        "pause_workflow"
    };
    match kind {
        "daemon" | "supervisor" => vec![workflow_toggle],
        "workflow_automation" | "agent_automation" | "background_processes" => {
            vec![workflow_toggle, "hard_reset_worktree"]
        }
        "agent" => vec!["stop_agent"],
        "chat" if process.get("session_id").and_then(Value::as_str).is_some() => {
            vec!["stop_agent"]
        }
        "interactive_session" if process.get("provider").and_then(Value::as_str).is_some() => {
            vec!["stop_agent"]
        }
        _ => Vec::new(),
    }
}

fn process_pause_state_from_summary(summary: &JsonObject) -> ProcessPauseState {
    ProcessPauseState {
        workflow_paused: summary
            .get("workflow_paused")
            .and_then(Value::as_bool)
            .or_else(|| summary.get("paused").and_then(Value::as_bool))
            .unwrap_or_else(|| {
                summary
                    .get("background_processes_stopped")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || summary
                        .get("agents_paused")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            }),
        disabled_background_workers: summary
            .get("disabled_background_workers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    }
}

fn apply_pause_state_aliases(summary: &mut JsonObject, pause_state: &ProcessPauseState) {
    for key in [
        "workflow_paused",
        "paused",
        "background_processes_stopped",
        "agents_paused",
    ] {
        summary.insert(key.to_string(), json!(pause_state.workflow_paused));
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

fn background_worker_values(processes: &[Value], pause_state: &ProcessPauseState) -> Vec<Value> {
    let mut worker_kinds = BACKGROUND_RUNNERS
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let known_worker_kinds = worker_kinds.iter().cloned().collect::<BTreeSet<_>>();
    let mut discovered_worker_kinds = pause_state.disabled_background_workers.clone();
    discovered_worker_kinds.extend(processes.iter().filter_map(|process| {
        (process.get("kind").and_then(Value::as_str) == Some("runner"))
            .then(|| process.get("worker_kind").and_then(Value::as_str))
            .flatten()
            .map(str::to_string)
    }));
    worker_kinds.extend(
        discovered_worker_kinds
            .difference(&known_worker_kinds)
            .cloned(),
    );

    worker_kinds
        .into_iter()
        .map(|worker_kind| {
            let process = processes.iter().find(|process| {
                process.get("kind").and_then(Value::as_str) == Some("runner")
                    && process.get("worker_kind").and_then(Value::as_str)
                        == Some(worker_kind.as_str())
                    && process.get("status").and_then(Value::as_str) == Some("running")
            });
            let disabled = pause_state
                .disabled_background_workers
                .contains(worker_kind.as_str());
            let globally_paused = pause_state.workflow_paused
                && matches!(worker_kind.as_str(), "git-sync" | "worktree-cleanup");
            let status = if process.is_some() {
                "running"
            } else if disabled {
                "stopped"
            } else if globally_paused {
                "paused"
            } else {
                "stopped"
            };
            let mut management_actions = vec![if process.is_some() {
                "stop_background_worker"
            } else {
                "start_background_worker"
            }];
            if worker_kind == "workflow" {
                management_actions.push(if pause_state.workflow_paused {
                    "unpause_workflow"
                } else {
                    "pause_workflow"
                });
            }
            json!({
                "id": format!("background-worker-{worker_kind}"),
                "kind": "background_worker",
                "worker_kind": &worker_kind,
                "label": &worker_kind,
                "status": status,
                "pid": process.and_then(|process| process.get("pid")).cloned().unwrap_or(Value::Null),
                "process_id": process.and_then(|process| process.get("id")).cloned().unwrap_or(Value::Null),
                "details": if disabled {
                    "stopped by user"
                } else if globally_paused {
                    "waiting for workflow automation to resume"
                } else if process.is_some() {
                    "supervised background worker"
                } else {
                    "not running"
                },
                "disabled": disabled,
                "management_actions": management_actions
            })
        })
        .collect()
}

fn runner_work_summary(runtime_root: &Path) -> Value {
    let status = "idle";
    let operations = FileOperationRegistry::new(runtime_root)
        .recover()
        .unwrap_or_default();
    let governance_integration_operation = operations
        .iter()
        .rev()
        .find(|operation| operation.owner.starts_with("governance-integration:"));
    let plan_extract_operation = operations
        .iter()
        .rev()
        .find(|operation| operation.owner == "import:extract:plan");
    let governance_integration_status = governance_integration_operation
        .map(|operation| operation.state.as_api_status().to_string())
        .unwrap_or_else(|| "idle".to_string());
    let governance_integration_goal_id = governance_integration_operation
        .and_then(|operation| operation.owner.strip_prefix("governance-integration:"))
        .map(ToString::to_string);
    let plan_extract_status = plan_extract_operation
        .map(|operation| operation.state.as_api_status().to_string())
        .unwrap_or_else(|| "idle".to_string());
    let plan_extract_details = plan_extract_operation
        .and_then(|operation| operation.progress.get("message").and_then(Value::as_str))
        .unwrap_or("Plan Draft extraction is ready for Draft Feature requests");
    let mut rows = [
        (
            "governance_integrator",
            "Governance integration coordinator",
        ),
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
        if kind == "governance_integrator" {
            json!({
                "kind": kind,
                "status": governance_integration_status,
                "elapsed_seconds": 0,
                "queued": 0,
                "details": details,
                "operation_id": governance_integration_operation.map(|operation| operation.id.clone()),
                "goal_id": governance_integration_goal_id
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

#[cfg(test)]
mod tests;
