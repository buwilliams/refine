use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_project_status(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("read project status");
        };
        match service.status() {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_path(&self, raw_path: &str) -> ApiResponse {
        let path = query_param(raw_path, "path").unwrap_or_default();
        let resolved = resolve_project_utility_path(&path);
        ApiResponse::json(
            200,
            json!({
                "path": resolved.display().to_string(),
                "input": path,
                "exists": resolved.exists(),
                "is_dir": resolved.is_dir(),
                "parent": resolved.parent().map(|path| path.display().to_string())
            }),
        )
    }

    pub(crate) fn handle_project_directories(&self, raw_path: &str) -> ApiResponse {
        let path = query_param(raw_path, "path").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(200)
            .clamp(1, 1000);
        match project_directories_response(&path, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_list(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("list projects");
        };
        match service.list_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_attach(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("attach projects");
        };
        let Some(path) = request
            .body
            .as_ref()
            .and_then(|body| body.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "path is required"
                    }
                }),
            );
        };
        self.stop_target_app_for_project_change();
        match service.attach_with_migration(path) {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_migrate(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("migrate project");
        };
        match service.migrate_current() {
            Ok(report) => ApiResponse::json(200, serde_json::to_value(report).unwrap()),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_register(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("register projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(path) = body
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput("path is required".to_string()));
        };
        let name = body
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::trim);
        match service.register_path(name, path, false) {
            Ok(registry) => ApiResponse::json(
                201,
                json!({
                    "ok": true,
                    "apps": registry_apps_array(&registry),
                    "current": registry.active_app.unwrap_or_default(),
                    "registry_enabled": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_clone(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("clone projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(source) = body
            .get("source")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput("source is required".to_string()));
        };
        let Some(destination) = body
            .get("destination")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "destination is required".to_string(),
            ));
        };
        let name = body
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty());
        let make_current = body
            .get("make_current")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        match service.clone_app(source, destination, name, make_current) {
            Ok(status) => ApiResponse::json(201, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_switch(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("switch projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(name) = body
            .get("name")
            .or_else(|| body.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "name or path is required".to_string(),
            ));
        };
        self.stop_target_app_for_project_change();
        match service.switch_with_migration(name) {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_detach(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("detach project");
        };
        self.stop_target_app_for_project_change();
        match service.detach() {
            Ok(status) => {
                if let Err(error) = self.refresh_runtime_projection_cache() {
                    return error_response(error);
                }
                ApiResponse::json(200, project_status_value(status))
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn stop_target_app_for_project_change(&self) {
        if self.current_target_root().ok().flatten().is_some() {
            let _ = self.target_app_service().and_then(|service| service.stop());
        }
        let Some(runtime_root) = &self.runtime_root else {
            return;
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        if let Ok(processes) = supervisor.recover_owner(ProcessOwner::TargetApp) {
            for process in processes
                .into_iter()
                .filter(|process| process.owner == ProcessOwner::TargetApp)
            {
                let _ = supervisor.signal(&process.id, "stop");
            }
        }
    }

    pub(crate) fn handle_project_remove(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("remove projects");
        };
        let Some(path) = request
            .body
            .as_ref()
            .and_then(|body| body.get("path").or_else(|| body.get("name")))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "path is required"
                    }
                }),
            );
        };
        match service.remove(path) {
            Ok(registry) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "apps": registry_apps_array(&registry),
                    "current": registry.active_app.unwrap_or_default(),
                    "registry_enabled": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_sync(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("synchronize Refine state");
        };
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("synchronize Refine state"),
            Err(error) => return error_response(error),
        };
        match FileRunnerWorkerService::new(runtime_root).queue_project_sync(&target_root) {
            Ok(operation) => {
                ApiResponse::json(202, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }
}
