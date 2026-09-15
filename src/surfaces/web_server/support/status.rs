use std::path::Path;

use serde_json::{Value, json};

#[cfg(test)]
pub(in crate::surfaces::web_server) use crate::application::diagnostics::processes::runtime_process_status_value;
pub(in crate::surfaces::web_server) use crate::application::diagnostics::processes::{
    process_status_value, process_summary_value_with_chat_sessions, runtime_process_summary_value,
};
use crate::application::projects::registry::registry_apps_array;
use crate::application::system::installation::InstallTarget;
use crate::error::RefineResult;
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::infrastructure::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::infrastructure::process::subprocess::ManagedProcess;
use crate::infrastructure::process::supervisor::operations::OperationHandle;
use crate::model::JsonObject;

use super::super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::surfaces::web_server) struct RuntimeReconcileSummary {
    pub(in crate::surfaces::web_server) processes: usize,
    pub(in crate::surfaces::web_server) operations: usize,
}

pub(in crate::surfaces::web_server) fn runtime_record_matches(
    process: &ManagedProcess,
    feature_id: &str,
    goal_ids: &[String],
) -> bool {
    process_text_matches(process.label.as_deref(), feature_id, goal_ids)
        || process_text_matches(process.details.as_deref(), feature_id, goal_ids)
}

pub(in crate::surfaces::web_server) fn operation_owner_matches(
    owner: &str,
    feature_id: &str,
    goal_ids: &[String],
) -> bool {
    process_text_matches(Some(owner), feature_id, goal_ids)
}

pub(in crate::surfaces::web_server) fn process_text_matches(
    text: Option<&str>,
    feature_id: &str,
    goal_ids: &[String],
) -> bool {
    let Some(text) = text else {
        return false;
    };
    text.contains(feature_id) || goal_ids.iter().any(|goal_id| text.contains(goal_id))
}

pub(in crate::surfaces::web_server) fn target_root_unavailable(action: &str) -> ApiResponse {
    ApiResponse::json(
        503,
        json!({
            "error": {
                "code": "target_root_unavailable",
                "message": format!("daemon cannot {action} without a target root")
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

pub(in crate::surfaces::web_server) fn operation_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Operation route requires an operation id"
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

pub(in crate::surfaces::web_server) fn agent_provider_from_path(
    path: &str,
    suffix: &str,
) -> Option<String> {
    path.strip_prefix("/agents/")
        .and_then(|path| path.strip_suffix(&format!("/{suffix}")))
        .map(str::trim)
        .filter(|provider| !provider.is_empty() && !provider.contains('/'))
        .map(super::percent_decode)
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

pub(in crate::surfaces::web_server) fn operation_response(
    operation: OperationHandle,
) -> serde_json::Value {
    json!({
        "id": operation.id,
        "schema_version": operation.schema_version,
        "revision": operation.revision,
        "owner": operation.owner,
        "status": operation.state.as_api_status(),
        "state": operation.state,
        "progress": operation.progress,
        "result": operation.result,
        "error": operation.error,
        "external_attempt": operation.external_attempt
    })
}

impl super::super::InProcessWebServer {
    pub(in crate::surfaces::web_server) fn provider_status_value(&self) -> RefineResult<Value> {
        let service = self.agent_provider_service()?;
        let selected_id = service.selected_provider_id("")?;
        let providers = service.detect()?;
        let selected = providers.iter().find(|p| p.name == selected_id);
        let ok = selected.is_some_and(|p| p.installed);
        let message = if ok {
            format!("{selected_id} CLI detected")
        } else {
            format!(
                "Configured AI provider {selected_id} is unavailable; install its executable on this host or edit Settings > Runtime"
            )
        };
        Ok(
            json!({"ok":ok,"stage":"provider_detection","message":message,
            "selected_provider":selected_id,"providers":providers}),
        )
    }
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
        "macos" | "macos_daemon" | "macos-daemon" => InstallTarget::MacosDaemon,
        "windows" | "windows_daemon" | "windows-daemon" => InstallTarget::WindowsDaemon,
        "linux" | "linux_cli_web" | "linux-cli-web" | "cli_web" => InstallTarget::LinuxCliWeb,
        _ => match std::env::consts::OS {
            "macos" => InstallTarget::MacosDaemon,
            "windows" => InstallTarget::WindowsDaemon,
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
        "target_root": status.target_root,
        "refine_dir": status.refine_dir,
        "config_path": status.config_path,
        "schema": status.schema,
        "maintenance": status.maintenance,
        "apps": apps,
        "active_node_id": status.active_node_id,
        "active_node": status.active_node,
        "active_node_diagnostics": status.active_node_diagnostics,
        "nodes": [{
            "id": status.active_node_id.clone().unwrap_or_else(|| "default".to_string()),
            "display_name": status.active_node.clone().unwrap_or_else(|| "Default".to_string()),
            "active": true
        }],
        "message": status.message
    })
}
