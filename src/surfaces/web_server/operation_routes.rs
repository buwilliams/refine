use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::process::subprocess::{FileProcessSupervisor, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::process::supervisor::security::{NativeSecretStore, SecretStore};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::installation::{FileInstallationService, InstallationService};
use crate::tools::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::tools::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::workflow::{WorkflowAutomation, WorkflowEngine};

use super::support::*;
use super::*;

#[derive(Clone, Debug)]
struct DiagnosticsCacheEntry {
    value: Value,
}

static DIAGNOSTICS_CACHE: OnceLock<Mutex<BTreeMap<String, DiagnosticsCacheEntry>>> =
    OnceLock::new();

impl InProcessWebServer {
    pub(super) fn handle_operation_status(&self, request: ApiRequest) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read background operations");
        }
        let Some(operation_id) = request
            .path
            .strip_prefix("/operations/")
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        match self.current_projection_with_runtime() {
            Ok(projection) => projection
                .runtime
                .background_operations
                .into_iter()
                .find(|operation| {
                    operation.get("id").and_then(|value| value.as_str()) == Some(operation_id)
                })
                .map(|operation| ApiResponse::json(200, json!({"operation": operation})))
                .unwrap_or_else(|| {
                    error_response(RefineError::NotFound(format!(
                        "Operation {operation_id} was not found"
                    )))
                }),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_operation_logs(&self, request: ApiRequest, raw_path: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read background operation logs");
        };
        let Some(operation_id) = request
            .path
            .strip_prefix("/operations/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 200);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        match FileOperationRegistry::new(runtime_root).page_logs(operation_id, limit, offset) {
            Ok((logs, has_more, total)) => {
                let log_count = logs.len();
                ApiResponse::json(
                    200,
                    json!({
                        "operation_id": operation_id,
                        "logs": logs,
                        "log_count": log_count,
                        "has_more": has_more,
                        "total": total,
                        "page": {
                            "limit": limit,
                            "offset": offset,
                            "has_more": has_more,
                            "total": total
                        }
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_operation_cancel(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("cancel background operations");
        };
        let Some(operation_id) = request
            .path
            .strip_prefix("/operations/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        match FileOperationRegistry::new(runtime_root).cancel(operation_id) {
            Ok(operation) => {
                let operation = operation_response(operation);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, json!({"operation": operation}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_workflow_execution_retry(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("retry workflow executions");
        };
        let Some(execution_id) = request
            .path
            .strip_prefix("/workflow/executions/")
            .and_then(|path| path.strip_suffix("/retry"))
            .filter(|execution_id| !execution_id.is_empty() && !execution_id.contains('/'))
        else {
            return operation_id_required();
        };
        let automation = match self.current_refine_dir() {
            Ok(Some(durable_root)) => WorkflowEngine::with_durable_root(runtime_root, durable_root),
            Ok(None) => WorkflowEngine::new(runtime_root),
            Err(error) => return error_response(error),
        };
        workflow_retry_response(&automation, execution_id)
    }

    pub(super) fn handle_workflow_execution_cancel(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("cancel workflow executions");
        };
        let Some(execution_id) = request
            .path
            .strip_prefix("/workflow/executions/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|execution_id| !execution_id.is_empty() && !execution_id.contains('/'))
        else {
            return operation_id_required();
        };
        let automation = match self.current_refine_dir() {
            Ok(Some(durable_root)) => WorkflowEngine::with_durable_root(runtime_root, durable_root),
            Ok(None) => WorkflowEngine::new(runtime_root),
            Err(error) => return error_response(error),
        };
        match automation.cancel(execution_id) {
            Ok(()) => match workflow_execution_json(&automation, execution_id) {
                Ok(execution) => ApiResponse::json(200, json!({"execution": execution})),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_processes(&self, raw_path: &str) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read managed processes");
        }
        let durable_root = match self.current_refine_dir() {
            Ok(root) => root,
            Err(error) => return error_response(error),
        };
        match self.current_projection_with_runtime() {
            Ok(projection) => ApiResponse::json(
                200,
                if query_param(raw_path, "summary").as_deref() == Some("1") {
                    runtime_process_status_value(&projection.runtime)
                } else {
                    let Some(runtime_root) = &self.runtime_root else {
                        return runtime_root_unavailable("read managed processes");
                    };
                    match process_summary_value_with_chat_sessions(
                        runtime_root,
                        durable_root.as_deref(),
                    ) {
                        Ok(value) => value,
                        Err(error) => return error_response(error),
                    }
                },
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_process_stream(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("stream managed process output");
        };
        let Some(process_id) = request
            .path
            .strip_prefix("/processes/")
            .and_then(|path| path.strip_suffix("/stream"))
            .filter(|process_id| !process_id.is_empty() && !process_id.contains('/'))
        else {
            return process_id_required();
        };
        match FileProcessSupervisor::new(runtime_root).stream(process_id) {
            Ok(output) => ApiResponse::json(
                200,
                json!({
                    "process_id": process_id,
                    "output": output,
                    "backend": {
                        "process_model": "supervisor"
                    }
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_processes_background(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control background processes");
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let current = match supervisor.pause_state() {
            Ok(state) => state,
            Err(error) => return error_response(error),
        };
        let stopped = request
            .body
            .as_ref()
            .and_then(|body| body.get("stopped"))
            .and_then(|stopped| stopped.as_bool())
            .unwrap_or(!current.background_processes_stopped);
        match supervisor.set_background_processes_stopped(stopped) {
            Ok(_) => {
                if stopped {
                    let durable_root = match self.current_refine_dir() {
                        Ok(root) => root,
                        Err(error) => return error_response(error),
                    };
                    if let Some(durable_root) = durable_root
                        && let Err(error) =
                            WorkflowEngine::with_durable_root(runtime_root, durable_root)
                                .rollback_in_progress_gaps_to_todo()
                    {
                        return error_response(error);
                    }
                }
                self.handle_processes("/processes")
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_processes_agents(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control agent processes");
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let current = match supervisor.pause_state() {
            Ok(state) => state,
            Err(error) => return error_response(error),
        };
        let paused = request
            .body
            .as_ref()
            .and_then(|body| body.get("paused"))
            .and_then(|paused| paused.as_bool())
            .unwrap_or(!current.agents_paused);
        match supervisor.set_agents_paused(paused) {
            Ok(_) => {
                if paused {
                    let durable_root = match self.current_refine_dir() {
                        Ok(root) => root,
                        Err(error) => return error_response(error),
                    };
                    if let Some(durable_root) = durable_root
                        && let Err(error) =
                            WorkflowEngine::with_durable_root(runtime_root, durable_root)
                                .rollback_in_progress_gaps_to_todo()
                    {
                        return error_response(error);
                    }
                }
                self.handle_processes("/processes")
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_install_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).status() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_install(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("install Refine");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let target = parse_install_target(body.get("target").and_then(|value| value.as_str()));
        let version = body
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        match FileInstallationService::new(runtime_root, version).install(target) {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_install_repair(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("repair install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).repair() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_install_update(&self, _request: ApiRequest) -> ApiResponse {
        error_response(RefineError::NotImplemented(
            "HTTP system update is disabled; run `./r system update` from the Refine checkout so the installer can stop daemons, update the deployed binary, refresh service metadata, and restart ports.".to_string(),
        ))
    }

    pub(super) fn handle_install_rollback(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rollback install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).rollback() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_install_uninstall(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("uninstall Refine");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).uninstall() {
            Ok(()) => ApiResponse::json(200, json!({"uninstalled": true})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_agents(&self) -> ApiResponse {
        provider_status_response()
    }

    pub(super) fn handle_agent_diagnostics(&self, request: ApiRequest) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "diagnostics") else {
            return provider_id_required();
        };
        match HostAgentProviderService::new().diagnose(provider) {
            Ok(diagnostics) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "diagnostics": diagnostics
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_agent_configure(&self, request: ApiRequest) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "configure") else {
            return provider_id_required();
        };
        match HostAgentProviderService::new().configure(provider) {
            Ok(()) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "configured": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_agent_invoke(&self, request: ApiRequest) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "invoke") else {
            return provider_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(prompt) = body.get("prompt").and_then(Value::as_str) else {
            return error_response(RefineError::InvalidInput(
                "agent invoke requires prompt".to_string(),
            ));
        };
        let cwd = body
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        match HostAgentProviderService::new().invoke(ProviderInvocation {
            provider: provider.to_string(),
            prompt: prompt.to_string(),
            session_id: None,
            cwd,
            process_metadata: Default::default(),
        }) {
            Ok(output) => ApiResponse::json(200, json!({"ok": true, "output": output})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_agent_resume(&self, request: ApiRequest) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "resume") else {
            return provider_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(session_id) = body.get("session_id").and_then(Value::as_str) else {
            return error_response(RefineError::InvalidInput(
                "agent resume requires session_id".to_string(),
            ));
        };
        match HostAgentProviderService::new().resume(provider, session_id) {
            Ok(output) => ApiResponse::json(200, json!({"ok": true, "output": output})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_agent_authenticate(&self, request: ApiRequest) -> ApiResponse {
        let suffix = if request.path.ends_with("/authenticate") {
            "authenticate"
        } else {
            "auth"
        };
        let Some(provider) = agent_provider_from_path(&request.path, suffix) else {
            return provider_id_required();
        };
        match HostAgentProviderService::new().authenticate(provider) {
            Ok(()) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "authenticated": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_recheck_auth(&self) -> ApiResponse {
        provider_status_response_refresh()
    }

    pub(super) fn handle_agent_secrets_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("inspect secret storage");
        };
        let store = NativeSecretStore::new(runtime_root);
        ApiResponse::json(200, json!({"secret_store": store.backend_status()}))
    }

    pub(super) fn handle_agent_secrets_list(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("list secrets");
        };
        let store = NativeSecretStore::new(runtime_root);
        match store.list_secrets() {
            Ok(secrets) => ApiResponse::json(200, json!({"secrets": secrets})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_agent_secret(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("manage secrets");
        };
        let Some((scope, name)) = secret_scope_name_from_path(&request.path) else {
            return error_response(RefineError::InvalidInput(
                "secret path must be /agents/secrets/{scope}/{name}".to_string(),
            ));
        };
        let store = NativeSecretStore::new(runtime_root);
        match request.method.as_str() {
            "GET" => match store.get_secret(&scope, &name) {
                Ok(secret) => ApiResponse::json(
                    200,
                    json!({"secret": secret.metadata, "value": secret.value}),
                ),
                Err(error) => error_response(error),
            },
            "PUT" | "POST" => {
                let body = request.body.unwrap_or_else(|| json!({}));
                let value = body
                    .get("value")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                match store.put_secret(&scope, &name, value) {
                    Ok(secret) => ApiResponse::json(200, json!({"secret": secret})),
                    Err(error) => error_response(error),
                }
            }
            "DELETE" => match store.delete_secret(&scope, &name) {
                Ok(secret) => ApiResponse::json(200, json!({"deleted": secret})),
                Err(error) => error_response(error),
            },
            _ => ApiResponse::json(
                405,
                json!({
                    "error": {
                        "code": "method_not_allowed",
                        "message": "secret route supports GET, PUT, POST, and DELETE"
                    }
                }),
            ),
        }
    }

    pub(super) fn handle_diagnostics(&self) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read diagnostics");
        }
        match self.diagnostics_value(false) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_support_bundle(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_refine_dir!(self, "export support bundle");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("export support bundle");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let redact_secrets = body
            .get("redact_secrets")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let repo_root = self
            .source_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        match FileSupportBundleService::new(durable_root, runtime_root.clone(), repo_root)
            .export(redact_secrets)
        {
            Ok(bundle) => ApiResponse::json(200, json!(bundle)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn warm_diagnostics_cache(&self) -> RefineResult<()> {
        if self.runtime_root.is_none() {
            return Ok(());
        }
        self.diagnostics_value(true).map(|_| ())
    }

    fn diagnostics_value(&self, refresh: bool) -> RefineResult<Value> {
        let Some(runtime_root) = &self.runtime_root else {
            return Err(RefineError::InvalidInput(
                "runtime root is required to read diagnostics".to_string(),
            ));
        };
        let repo_root = self
            .source_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let durable_root = self.current_refine_dir().ok().flatten();
        let cache_key = diagnostics_cache_key(runtime_root, durable_root.as_ref(), &repo_root);
        if !refresh {
            let cache = DIAGNOSTICS_CACHE
                .get_or_init(|| Mutex::new(BTreeMap::new()))
                .lock()
                .map_err(|_| RefineError::Io("diagnostics cache lock was poisoned".to_string()))?;
            if let Some(entry) = cache.get(&cache_key) {
                return Ok(entry.value.clone());
            }
        }
        let projection = match self.current_projection_with_runtime() {
            Ok(projection) => projection,
            Err(error) => return Err(error),
        };
        let doctor =
            FileDiagnosticsService::new(durable_root, runtime_root.clone(), repo_root).doctor()?;
        let provider = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        let process = runtime_process_status_value(&projection.runtime);
        let value = json!({
            "reachable": true,
            "backend": {
                "process_model": "supervisor",
                "native": true
            },
            "provider": provider,
            "processes": process,
            "doctor": doctor
        });
        DIAGNOSTICS_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| RefineError::Io("diagnostics cache lock was poisoned".to_string()))?
            .insert(
                cache_key,
                DiagnosticsCacheEntry {
                    value: value.clone(),
                },
            );
        Ok(value)
    }
}

fn workflow_retry_response(automation: &WorkflowEngine, execution_id: &str) -> ApiResponse {
    match automation.retry(execution_id) {
        Ok(retried_execution_id) => {
            match workflow_execution_json(automation, &retried_execution_id) {
                Ok(execution) => ApiResponse::json(
                    200,
                    json!({
                        "retried_from": execution_id,
                        "execution": execution
                    }),
                ),
                Err(error) => error_response(error),
            }
        }
        Err(error) => error_response(error),
    }
}

fn workflow_execution_json(automation: &WorkflowEngine, execution_id: &str) -> RefineResult<Value> {
    let state = automation.load_state()?;
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.execution_id.as_deref() == Some(execution_id))
        .ok_or_else(|| {
            RefineError::NotFound(format!("Workflow execution {execution_id} was not found"))
        })?;
    Ok(json!({
        "id": execution_id,
        "claim_id": claim.claim_id,
        "gap_id": claim.gap_id,
        "status": claim.state,
        "node_id": claim.node_id,
        "provider": claim.provider,
        "target_app_id": claim.target_app_id,
        "created_at": claim.created_at,
        "updated_at": claim.updated_at
    }))
}

fn diagnostics_cache_key(
    runtime_root: &std::path::Path,
    durable_root: Option<&PathBuf>,
    repo_root: &std::path::Path,
) -> String {
    format!(
        "{}|{}|{}",
        runtime_root.display(),
        durable_root
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string()),
        repo_root.display()
    )
}

fn secret_scope_name_from_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/agents/secrets/")?;
    let mut parts = rest.split('/');
    let scope = parts.next()?.trim();
    let name = parts.next()?.trim();
    if scope.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((scope.to_string(), name.to_string()))
}
