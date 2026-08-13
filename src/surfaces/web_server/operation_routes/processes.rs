use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_processes(&self, raw_path: &str) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read managed processes");
        }
        match self.current_runtime_projection() {
            Ok(runtime) => {
                let mut summary = runtime_process_summary_value(&runtime);
                enrich_process_resource_usage(&mut summary);
                if let Some(object) = summary.as_object_mut() {
                    object.insert(
                        "target_app".to_string(),
                        runtime
                            .target_app
                            .clone()
                            .map(Value::Object)
                            .unwrap_or(Value::Null),
                    );
                    let daemon = self
                        .product_paths
                        .as_ref()
                        .map(|paths| repository_disk_usage_value(&paths.checkout));
                    let target_app = self
                        .current_target_root()
                        .ok()
                        .flatten()
                        .map(|root| repository_disk_usage_value(&root));
                    object.insert(
                        "repository_disk_usage".to_string(),
                        json!({"daemon": daemon, "target_app": target_app}),
                    );
                }
                ApiResponse::json(
                    200,
                    if query_param(raw_path, "summary").as_deref() == Some("1") {
                        process_status_value(&summary)
                    } else {
                        summary
                    },
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_process_stream(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
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
        match FileProcessStatusService::new(runtime_root).stream(process_id) {
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

    pub(in crate::surfaces::web_server) fn handle_process_stop(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("stop managed process");
        };
        let Some(process_id) = request
            .path
            .strip_prefix("/processes/")
            .and_then(|path| path.strip_suffix("/stop"))
            .filter(|process_id| !process_id.is_empty() && !process_id.contains('/'))
        else {
            return process_id_required();
        };
        let signal = request
            .body
            .as_ref()
            .and_then(|body| body.get("signal"))
            .and_then(|signal| signal.as_str())
            .unwrap_or("terminate");
        let service = match self.current_refine_dir() {
            Ok(Some(refine_dir)) => {
                FileProcessControlService::with_refine_dir(runtime_root, refine_dir)
            }
            Ok(None) => FileProcessControlService::new(runtime_root),
            Err(error) => return error_response(error),
        };
        match service.stop(process_id, signal) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_background_worker_control(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control background worker");
        };
        let Some(remainder) = request.path.strip_prefix("/processes/background-workers/") else {
            return process_id_required();
        };
        let mut parts = remainder.split('/');
        let Some(worker_kind) = parts.next().filter(|value| !value.is_empty()) else {
            return process_id_required();
        };
        let Some(action) = parts.next().filter(|value| !value.is_empty()) else {
            return process_id_required();
        };
        if parts.next().is_some() || !matches!(action, "start" | "stop") {
            return error_response(RefineError::InvalidInput(
                "background worker action must be start or stop".to_string(),
            ));
        }

        let mut workers = FileRunnerWorkerService::new(runtime_root);
        if let Some(registry_root) = self.app_registry_runtime_root() {
            workers = workers.with_project_registry_root(registry_root);
        }
        match workers.set_background_worker_enabled(worker_kind, action == "start") {
            Ok(BackgroundWorkerEnsure::Running(process)) => ApiResponse::json(
                200,
                json!({"worker_kind": worker_kind, "status": "running", "process": process.api_json()}),
            ),
            Ok(BackgroundWorkerEnsure::Paused) => {
                ApiResponse::json(200, json!({"worker_kind": worker_kind, "status": "paused"}))
            }
            Ok(BackgroundWorkerEnsure::Disabled) => ApiResponse::json(
                200,
                json!({"worker_kind": worker_kind, "status": "stopped"}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_processes_background(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control workflow automation");
        };
        let current = match FileProcessSupervisor::new(runtime_root).pause_state() {
            Ok(state) => state,
            Err(error) => return error_response(error),
        };
        let paused = request
            .body
            .as_ref()
            .and_then(|body| body.get("stopped"))
            .and_then(|stopped| stopped.as_bool())
            .unwrap_or(!current.workflow_paused);
        self.set_workflow_paused_response(paused)
    }

    pub(in crate::surfaces::web_server) fn handle_processes_agents(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control workflow automation");
        };
        let current = match FileProcessSupervisor::new(runtime_root).pause_state() {
            Ok(state) => state,
            Err(error) => return error_response(error),
        };
        let paused = request
            .body
            .as_ref()
            .and_then(|body| body.get("paused"))
            .and_then(|paused| paused.as_bool())
            .unwrap_or(!current.workflow_paused);
        self.set_workflow_paused_response(paused)
    }
}
