use std::path::PathBuf;

use serde_json::{Value, json};

use crate::core::host::agent_providers::{AgentProviderService, HostAgentProviderService};
use crate::core::host::installation::{FileInstallationService, InstallationService};
use crate::core::host::process_supervision::{FileProcessSupervisor, ProcessSupervisor};
use crate::core::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::core::product::scheduling::{FileSchedulingService, SchedulingService};
use crate::core::supervisor::errors::RefineError;
use crate::core::supervisor::jobs::{FileJobRegistry, JobRegistry};

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn handle_job_status(&self, request: ApiRequest) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read background jobs");
        }
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        match self.current_projection_with_runtime() {
            Ok(projection) => projection
                .runtime
                .background_jobs
                .into_iter()
                .find(|job| job.get("id").and_then(|value| value.as_str()) == Some(job_id))
                .map(|job| ApiResponse::json(200, json!({"job": job})))
                .unwrap_or_else(|| {
                    error_response(RefineError::NotFound(format!("Job {job_id} was not found")))
                }),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_job_logs(&self, request: ApiRequest, raw_path: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read background job logs");
        };
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 200);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        match FileJobRegistry::new(runtime_root).page_logs(job_id, limit, offset) {
            Ok((logs, has_more, total)) => {
                let log_count = logs.len();
                ApiResponse::json(
                    200,
                    json!({
                        "job_id": job_id,
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

    pub(super) fn handle_job_cancel(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("cancel background jobs");
        };
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        match FileJobRegistry::new(runtime_root).cancel(job_id) {
            Ok(job) => {
                let job = job_response(job);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, json!({"job": job}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_job_retry(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("retry background jobs");
        };
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .and_then(|path| path.strip_suffix("/retry"))
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        let scheduler = match self.current_durable_root() {
            Ok(Some(durable_root)) => {
                FileSchedulingService::with_durable_root(runtime_root, durable_root)
            }
            Ok(None) => FileSchedulingService::new(runtime_root),
            Err(error) => return error_response(error),
        };
        match scheduler.retry(job_id) {
            Ok(retried_job_id) => {
                match FileJobRegistry::new(runtime_root).status(&retried_job_id) {
                    Ok(job) => {
                        let job = job_response(job);
                        if let Err(error) = self.current_projection_with_runtime() {
                            return error_response(error);
                        }
                        ApiResponse::json(
                            200,
                            json!({
                                "retried_from": job_id,
                                "job": job
                            }),
                        )
                    }
                    Err(error) => error_response(error),
                }
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_processes(&self) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read managed processes");
        }
        match self.current_projection_with_runtime() {
            Ok(projection) => {
                ApiResponse::json(200, runtime_process_summary_value(&projection.runtime))
            }
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
            Ok(_) => self.handle_processes(),
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
            Ok(_) => self.handle_processes(),
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

    pub(super) fn handle_install_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("update install state");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(version) = body.get("version").and_then(|value| value.as_str()) else {
            return error_response(RefineError::InvalidInput("version is required".to_string()));
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).update(version)
        {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
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
        provider_status_response()
    }

    pub(super) fn handle_diagnostics(&self) -> ApiResponse {
        let projection = match self.current_projection_with_runtime() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read diagnostics");
        };
        let repo_root = self
            .source_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let doctor = match FileDiagnosticsService::new(
            self.current_durable_root().ok().flatten(),
            runtime_root.clone(),
            repo_root,
        )
        .doctor()
        {
            Ok(report) => report,
            Err(error) => return error_response(error),
        };
        let provider = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        let process = runtime_process_summary_value(&projection.runtime);
        ApiResponse::json(
            200,
            json!({
                "reachable": true,
                "backend": {
                    "process_model": "supervisor",
                    "native": true
                },
                "provider": provider,
                "processes": process,
                "doctor": doctor
            }),
        )
    }
}
