use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::core::host::agent_providers::{AgentProviderService, HostAgentProviderService};
use crate::core::host::installation::InstallTarget;
use crate::core::host::process_supervision::{FileProcessSupervisor, ManagedProcess, ProcessOwner};
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::core::product::chat::{ChatAttachment, ChatSessionRecord, FileChatService};
use crate::core::product::project_registry::registry_apps_array;
use crate::core::product::project_state::RuntimeProjection;
use crate::core::supervisor::errors::RefineResult;
use crate::core::supervisor::jobs::{FileJobRegistry, JobHandle, JobRegistry};
use crate::model::JsonObject;

use super::super::*;
use super::*;

const PROVIDER_STATUS_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
struct ProviderStatusCacheEntry {
    value: Value,
    refreshed_at: Instant,
}

static PROVIDER_STATUS_CACHE: OnceLock<Mutex<Option<ProviderStatusCacheEntry>>> = OnceLock::new();

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::surfaces::web_server) struct RuntimeReconcileSummary {
    pub(in crate::surfaces::web_server) processes: usize,
    pub(in crate::surfaces::web_server) jobs: usize,
}

pub(in crate::surfaces::web_server) fn runtime_record_matches(
    process: &ManagedProcess,
    feature_id: &str,
    gap_ids: &[String],
) -> bool {
    process_text_matches(process.label.as_deref(), feature_id, gap_ids)
        || process_text_matches(process.details.as_deref(), feature_id, gap_ids)
}

pub(in crate::surfaces::web_server) fn job_owner_matches(
    owner: &str,
    feature_id: &str,
    gap_ids: &[String],
) -> bool {
    process_text_matches(Some(owner), feature_id, gap_ids)
}

pub(in crate::surfaces::web_server) fn process_text_matches(
    text: Option<&str>,
    feature_id: &str,
    gap_ids: &[String],
) -> bool {
    let Some(text) = text else {
        return false;
    };
    text.contains(feature_id) || gap_ids.iter().any(|gap_id| text.contains(gap_id))
}

pub(in crate::surfaces::web_server) fn durable_root_unavailable(action: &str) -> ApiResponse {
    ApiResponse::json(
        503,
        json!({
            "error": {
                "code": "durable_root_unavailable",
                "message": format!("daemon cannot {action} without a durable root")
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn runtime_root_unavailable(action: &str) -> ApiResponse {
    ApiResponse::json(
        503,
        json!({
            "error": {
                "code": "runtime_root_unavailable",
                "message": format!("daemon cannot {action} without a runtime root")
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn job_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Job route requires a job id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn process_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Process route requires a process id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn provider_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Provider route requires a provider id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn agent_provider_from_path<'a>(
    path: &'a str,
    suffix: &str,
) -> Option<&'a str> {
    path.strip_prefix("/agents/")
        .and_then(|path| path.strip_suffix(&format!("/{suffix}")))
        .map(str::trim)
        .filter(|provider| !provider.is_empty() && !provider.contains('/'))
}

pub(in crate::surfaces::web_server) fn regression_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Regression route requires a regression id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn chat_session_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Chat route requires a session id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn reporter_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Reporter route requires a reporter id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn reporter_id_from_path(
    path: &str,
    prefix: &str,
    suffix: &str,
) -> Option<u64> {
    path.strip_prefix(prefix)
        .and_then(|path| {
            if suffix.is_empty() {
                Some(path)
            } else {
                path.strip_suffix(suffix)
            }
        })
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .and_then(|id| id.parse::<u64>().ok())
}

pub(in crate::surfaces::web_server) fn first_non_empty(first: &str, second: &str) -> String {
    if first.trim().is_empty() {
        second.to_string()
    } else {
        first.to_string()
    }
}

pub(in crate::surfaces::web_server) fn append_quality_activity(
    durable_root: &Path,
    message: String,
) {
    let service = FileActivityService::new(durable_root);
    let entry = service.new_entry(message, "info", "quality", None, Some("refine".to_string()));
    let _ = service.append(entry);
}

pub(in crate::surfaces::web_server) fn job_response(job: JobHandle) -> serde_json::Value {
    json!({
        "id": job.id,
        "owner": job.owner,
        "status": job.state.as_api_status(),
        "state": job.state,
        "progress": job.progress,
        "result": job.result,
        "error": job.error
    })
}

pub(in crate::surfaces::web_server) fn process_summary_response(
    runtime_root: &Path,
) -> ApiResponse {
    match process_summary_value(runtime_root) {
        Ok(value) => ApiResponse::json(200, value),
        Err(error) => error_response(error),
    }
}

pub(in crate::surfaces::web_server) fn process_summary_value(
    runtime_root: &Path,
) -> RefineResult<Value> {
    process_summary_value_with_chat_sessions(runtime_root, None)
}

pub(in crate::surfaces::web_server) fn process_summary_value_with_chat_sessions(
    runtime_root: &Path,
    durable_root: Option<&Path>,
) -> RefineResult<Value> {
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let pause_state = supervisor.pause_state()?;
    let mut process_values = Vec::new();
    let mut seen_process_ids = BTreeSet::new();
    for process_root in process_roots(runtime_root, durable_root) {
        let processes =
            FileProcessSupervisor::new(&process_root).recover_owner(ProcessOwner::TargetApp)?;
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
    append_chat_session_processes(&mut process_values, runtime_root, durable_root)?;
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

fn process_roots(runtime_root: &Path, durable_root: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![runtime_root.to_path_buf()];
    if let Some(durable_root) = durable_root {
        let project_runtime_root = durable_root.join("runtime");
        if project_runtime_root != runtime_root {
            roots.push(project_runtime_root);
        }
    }
    roots
}

fn append_chat_session_processes(
    process_values: &mut Vec<Value>,
    runtime_root: &Path,
    durable_root: Option<&Path>,
) -> RefineResult<()> {
    let Some(durable_root) = durable_root else {
        return Ok(());
    };
    let existing_session_ids = process_values
        .iter()
        .filter_map(|process| process.get("session_id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let service = FileChatService::with_runtime_root(durable_root, runtime_root);
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

fn is_current_process_object(process: &JsonObject) -> bool {
    let status = process.get("status").and_then(Value::as_str).unwrap_or("");
    !is_terminal_process_status(status)
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
    let merger_job = FileJobRegistry::new(runtime_root)
        .recover()
        .ok()
        .and_then(|jobs| {
            jobs.into_iter()
                .rev()
                .find(|job| job.owner.starts_with("merger:"))
        });
    let merger_status = if background_stopped {
        "paused".to_string()
    } else {
        merger_job
            .as_ref()
            .map(|job| job.state.as_api_status().to_string())
            .unwrap_or_else(|| "idle".to_string())
    };
    let merger_gap_id = merger_job
        .as_ref()
        .and_then(|job| job.owner.strip_prefix("merger:"))
        .map(ToString::to_string);
    let mut rows = [
        ("merger", "serial Gap branch merger"),
        (
            "target_app_rebuilder",
            "target-app rebuild worker is ready for manual rebuild requests",
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
                "job_id": merger_job.as_ref().map(|job| job.id.clone()),
                "gap_id": merger_gap_id
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

pub(in crate::surfaces::web_server) fn provider_status_response() -> ApiResponse {
    match provider_status_value() {
        Ok(value) => ApiResponse::json(200, value),
        Err(error) => error_response(error),
    }
}

pub(in crate::surfaces::web_server) fn provider_status_response_refresh() -> ApiResponse {
    match provider_status_value_refresh() {
        Ok(value) => ApiResponse::json(200, value),
        Err(error) => error_response(error),
    }
}

pub(in crate::surfaces::web_server) fn provider_status_value() -> RefineResult<Value> {
    cached_provider_status_value(false)
}

pub(in crate::surfaces::web_server) fn provider_status_value_refresh() -> RefineResult<Value> {
    cached_provider_status_value(true)
}

fn cached_provider_status_value(refresh: bool) -> RefineResult<Value> {
    let cache = PROVIDER_STATUS_CACHE.get_or_init(|| Mutex::new(None));
    let mut cache = cache.lock().map_err(|_| {
        crate::core::supervisor::errors::RefineError::Io(
            "provider status cache lock was poisoned".to_string(),
        )
    })?;
    if !refresh
        && let Some(entry) = cache.as_ref()
        && entry.refreshed_at.elapsed() < PROVIDER_STATUS_CACHE_TTL
    {
        return Ok(entry.value.clone());
    }
    let value = fresh_provider_status_value()?;
    *cache = Some(ProviderStatusCacheEntry {
        value: value.clone(),
        refreshed_at: Instant::now(),
    });
    Ok(value)
}

fn fresh_provider_status_value() -> RefineResult<Value> {
    let service = HostAgentProviderService::new();
    let providers = service.detect()?;
    let selected = providers
        .iter()
        .find(|provider| provider.installed)
        .or_else(|| providers.iter().find(|provider| provider.name == "claude"));
    let ok = selected.map(|provider| provider.installed).unwrap_or(false);
    let message = if ok {
        selected
            .map(|provider| format!("{} CLI detected", provider.display_name))
            .unwrap_or_else(|| "provider detected".to_string())
    } else {
        "No supported provider CLI detected on PATH".to_string()
    };
    Ok(json!({
        "ok": ok,
        "stage": "provider_detection",
        "message": message,
        "selected_provider": selected.map(|provider| provider.name.clone()).unwrap_or_else(|| "claude".to_string()),
        "providers": providers
    }))
}

pub(in crate::surfaces::web_server) fn performance_report_value(
    runtime_root: &Path,
    query: PerformanceQuery,
) -> RefineResult<Value> {
    let service = FileMetricsService::new(runtime_root);
    let report = service.report(query)?;
    Ok(json!({
        "summary": report.summary,
        "recent": report.recent,
        "events": report.events,
        "operations": report.operations,
        "event_count": report.event_count,
        "filtered_event_count": report.filtered_event_count,
        "total_event_count": report.total_event_count,
        "retention_days": report.retention_days,
        "page": report.page,
        "backend": {
            "process_model": "supervisor",
            "native": true,
            "store": "jsonl"
        }
    }))
}

pub(in crate::surfaces::web_server) fn runtime_process_summary_value(
    runtime: &RuntimeProjection,
) -> Value {
    let mut summary = runtime
        .supervisor
        .clone()
        .unwrap_or_else(|| serde_json::Map::new());
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

pub(in crate::surfaces::web_server) fn runtime_process_status_value(
    runtime: &RuntimeProjection,
) -> Value {
    let mut summary = runtime
        .supervisor
        .clone()
        .unwrap_or_else(|| serde_json::Map::new());
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

pub(in crate::surfaces::web_server) fn value_object(value: Value) -> Option<JsonObject> {
    match value {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

pub(in crate::surfaces::web_server) fn runtime_bool_setting(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().unwrap_or_default() != 0,
        Value::String(value) => {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        _ => false,
    }
}

pub(in crate::surfaces::web_server) fn parse_install_target(value: Option<&str>) -> InstallTarget {
    match value.unwrap_or("").trim().to_lowercase().as_str() {
        "macos" | "macos_app_bundle" | "macos-app-bundle" | "app_bundle" => {
            InstallTarget::MacOsAppBundle
        }
        "windows" | "windows_installer" | "windows-installer" | "installer" => {
            InstallTarget::WindowsInstaller
        }
        "linux" | "linux_cli_web" | "linux-cli-web" | "cli_web" => InstallTarget::LinuxCliWeb,
        _ => match std::env::consts::OS {
            "macos" => InstallTarget::MacOsAppBundle,
            "windows" => InstallTarget::WindowsInstaller,
            _ => InstallTarget::LinuxCliWeb,
        },
    }
}

pub(in crate::surfaces::web_server) fn project_status_value(
    status: crate::model::project::ProjectStatus,
) -> serde_json::Value {
    let apps = registry_apps_array(&status.apps);
    json!({
        "attached": status.attached,
        "registry_enabled": status.registry_enabled,
        "client_repo": status.client_repo,
        "volume_root": status.volume_root,
        "config_path": status.config_path,
        "schema": status.schema,
        "maintenance": status.maintenance,
        "apps": apps,
        "active_node_id": status.active_node_id,
        "active_node": status.active_node,
        "nodes": [{
            "id": status.active_node_id.clone().unwrap_or_else(|| "default".to_string()),
            "display_name": status.active_node.clone().unwrap_or_else(|| "Default".to_string()),
            "active": true
        }],
        "message": status.message
    })
}
