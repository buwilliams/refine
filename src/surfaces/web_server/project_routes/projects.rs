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

    /// `POST /sync`: without an authority body, queue the ordinary sync
    /// pipeline as a supervised operation; with `{authority, paths}`, run the
    /// terminal recovery — sync with a decision attached — synchronously.
    pub(crate) fn handle_sync(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        if body.get("authority").is_some() {
            return self.handle_sync_with_authority(&body);
        }
        if body.get("paths").is_some() {
            return error_response(RefineError::InvalidInput(
                "paths require an authority of live or remote".to_string(),
            ));
        }
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

    pub(crate) fn handle_sync_preview(&self) -> ApiResponse {
        let service = match self.current_git_sync_service() {
            Ok(Some(service)) => service,
            Ok(None) => return target_root_unavailable("preview state synchronization"),
            Err(error) => return error_response(error),
        };
        match service.preview_state_recovery() {
            Ok(preview) => ApiResponse::json(200, serde_json::to_value(preview).unwrap()),
            Err(error) => error_response(error),
        }
    }

    /// Named `paths` are exceptions settled on the opposite side of the
    /// chosen authority.
    fn handle_sync_with_authority(&self, body: &Value) -> ApiResponse {
        let (service, target_root) = match self.current_git_sync_service_with_target() {
            Ok(Some(service_and_target)) => service_and_target,
            Ok(None) => return target_root_unavailable("settle state synchronization"),
            Err(error) => return error_response(error),
        };
        use crate::tools::host::git_sync::{
            StateRecoveryAuthority, StateRecoveryDecision, StateRecoveryOverride,
            StateRecoveryRunPolicy,
        };
        let default_authority: StateRecoveryAuthority =
            match serde_json::from_value(body["authority"].clone()) {
                Ok(authority) => authority,
                Err(_) => {
                    return error_response(RefineError::InvalidInput(
                        "authority must be live or remote".to_string(),
                    ));
                }
            };
        let exception = match default_authority {
            StateRecoveryAuthority::Live => StateRecoveryAuthority::Remote,
            StateRecoveryAuthority::Remote => StateRecoveryAuthority::Live,
        };
        let paths: Vec<String> = match body.get("paths") {
            None => Vec::new(),
            Some(paths) => match serde_json::from_value(paths.clone()) {
                Ok(paths) => paths,
                Err(_) => {
                    return error_response(RefineError::InvalidInput(
                        "paths must be an array of contested path strings".to_string(),
                    ));
                }
            },
        };
        let policy = StateRecoveryRunPolicy::Decision(StateRecoveryDecision {
            default_authority,
            overrides: paths
                .into_iter()
                .map(|path| StateRecoveryOverride {
                    path,
                    authority: exception,
                })
                .collect(),
        });
        let recovery_health = self.current_state_sync_health().ok().flatten();
        match service.run_state_recovery_with_policy(policy) {
            Ok(result) => {
                let settlement = result
                    .recovered
                    .then_some(())
                    .and(self.runtime_root.as_deref())
                    .zip(recovery_health.as_ref())
                    .and_then(|(runtime_root, expected_health)| {
                        crate::process::runner::settle_state_recovery_success(
                            runtime_root,
                            &target_root,
                            expected_health,
                        )
                        .ok()
                    });
                let health_settled = settlement.as_ref().is_some_and(|(settled, _)| *settled);
                let state_sync_health = settlement.map(|(_, health)| health);
                let mut value = serde_json::to_value(result).unwrap();
                if let Value::Object(fields) = &mut value {
                    fields.insert("health_settled".to_string(), json!(health_settled));
                    fields.insert(
                        "state_sync_health".to_string(),
                        state_sync_health.map_or(Value::Null, |health| json!(health)),
                    );
                }
                // `/sync` is exempt from the blanket post-mutation refresh
                // (the queued form's worker rebuilds the projection itself),
                // so the synchronous terminal form refreshes here.
                if let Err(error) = self.refresh_projection_cache_after_mutation() {
                    return error_response(error);
                }
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_project_worktree_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("clean target-app worktrees");
        };
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("clean target-app worktrees"),
            Err(error) => return error_response(error),
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let options = WorktreeCleanupOptions {
            apply: body.get("apply").and_then(Value::as_bool).unwrap_or(false),
            older_than_seconds: body
                .get("older_than_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        };
        match FileWorktreeCleanupService::new(target_root, runtime_root).run(options) {
            Ok(report) => ApiResponse::json(200, serde_json::to_value(report).unwrap()),
            Err(error) => error_response(error),
        }
    }
}
