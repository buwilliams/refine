use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::core::host::agent_providers::{AgentProviderService, HostAgentProviderService};
use crate::core::host::installation::InstallTarget;
use crate::core::host::process_supervision::{FileProcessSupervisor, ManagedProcess};
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::core::product::project_registry::registry_apps_array;
use crate::core::product::project_state::RuntimeProjection;
use crate::core::supervisor::errors::RefineResult;
use crate::core::supervisor::jobs::JobHandle;
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
        "progress": {},
        "result": {},
        "error": null
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
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let pause_state = supervisor.pause_state()?;
    let processes = supervisor.list()?;
    let process_values: Vec<_> = processes
        .into_iter()
        .map(|process| process.api_json())
        .collect();
    Ok(json!({
        "runner_reachable": true,
        "paused": pause_state.background_processes_stopped || pause_state.agents_paused,
        "background_processes_stopped": pause_state.background_processes_stopped,
        "agents_paused": pause_state.agents_paused,
        "processes": process_values,
        "runner_work": [],
        "backend": {
            "process_model": "supervisor"
        }
    }))
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
