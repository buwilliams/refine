use super::*;

impl InProcessWebServer {
    fn system_runtime_root(&self) -> RefineResult<std::path::PathBuf> {
        if let Some(paths) = &self.product_paths {
            let expected_port_root = paths.port_runtime_root(self.status.port);
            if self.runtime_root.as_deref() != Some(expected_port_root.as_path())
                || self.app_registry_root.as_deref() != Some(paths.runtime_root.as_path())
            {
                return Err(RefineError::Conflict(format!(
                    "daemon runtime bootstrap does not match owning checkout {} and port {}",
                    paths.checkout.display(),
                    self.status.port
                )));
            }
            return Ok(paths.runtime_root.clone());
        }
        if let Some(root) = &self.app_registry_root {
            return Ok(root.clone());
        }
        let runtime_root = self.runtime_root.as_ref().ok_or_else(|| {
            RefineError::NotFound("daemon runtime root is unavailable".to_string())
        })?;
        if runtime_root.file_name().and_then(|name| name.to_str())
            == Some(self.status.port.to_string().as_str())
        {
            return runtime_root
                .parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "port-scoped runtime root has no parent: {}",
                        runtime_root.display()
                    ))
                });
        }
        Ok(runtime_root.clone())
    }

    fn system_installation(&self, version: &str) -> RefineResult<FileInstallationService> {
        Ok(FileInstallationService::for_port(
            self.system_runtime_root()?,
            version,
            self.status.port,
        ))
    }

    fn system_lifecycle(&self) -> RefineResult<FileHostDaemonLifecycleService> {
        Ok(FileHostDaemonLifecycleService::new(
            RuntimeRoot {
                root: self.system_runtime_root()?,
            },
            env!("CARGO_PKG_VERSION"),
        ))
    }

    fn system_lifecycle_operations(&self) -> RefineResult<FileDaemonLifecycleOperationService> {
        Ok(FileDaemonLifecycleOperationService::new(
            RuntimeRoot {
                root: self.system_runtime_root()?,
            },
            env!("CARGO_PKG_VERSION"),
        ))
    }

    pub(in crate::surfaces::web_server) fn handle_install_status(&self) -> ApiResponse {
        let service = match self.system_installation(env!("CARGO_PKG_VERSION")) {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        match service.status() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let target = parse_install_target(body.get("target").and_then(|value| value.as_str()));
        let version = body
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        let service = match self.system_installation(version) {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        match service.install(target) {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install_repair(&self) -> ApiResponse {
        let service = match self.system_installation(env!("CARGO_PKG_VERSION")) {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        match service.repair() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install_update(
        &self,
        _request: ApiRequest,
    ) -> ApiResponse {
        let service = match self.system_installation(env!("CARGO_PKG_VERSION")) {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        if let Err(error) = service.status() {
            return error_response(error);
        }
        error_response(RefineError::NotImplemented(
            "HTTP system update is disabled; use the Update control in the web UI or run `./r system update` from the Refine checkout — both queue the same restart-safe source promotion.".to_string(),
        ))
    }

    pub(in crate::surfaces::web_server) fn handle_daemon_lifecycle(
        &self,
        action: DaemonLifecycleAction,
    ) -> ApiResponse {
        if matches!(
            action,
            DaemonLifecycleAction::Stop | DaemonLifecycleAction::Restart
        ) {
            let operations = match self.system_lifecycle_operations() {
                Ok(operations) => operations,
                Err(error) => return error_response(error),
            };
            return match operations.queue(
                action,
                BackgroundDaemonConfig {
                    port: self.status.port,
                    ..Default::default()
                },
            ) {
                Ok(operation) => ApiResponse::json(202, json!({"operation": operation})),
                Err(error) => error_response(error),
            };
        }
        let lifecycle = match self.system_lifecycle() {
            Ok(lifecycle) => lifecycle,
            Err(error) => return error_response(error),
        };
        self.handle_daemon_lifecycle_with(action, &lifecycle)
    }

    pub(in crate::surfaces::web_server) fn handle_daemon_lifecycle_with(
        &self,
        action: DaemonLifecycleAction,
        lifecycle: &impl crate::tools::host::daemon_lifecycle::HostDaemonLifecycleService,
    ) -> ApiResponse {
        match execute_daemon_lifecycle(
            lifecycle,
            action,
            BackgroundDaemonConfig {
                port: self.status.port,
                ..Default::default()
            },
        ) {
            Ok(status) => ApiResponse::json(200, json!({"status": status})),
            Err(error) => error_response(error),
        }
    }

    #[cfg(test)]
    pub(in crate::surfaces::web_server) fn handle_daemon_lifecycle_handoff_with(
        &self,
        action: DaemonLifecycleAction,
        executable: &std::path::Path,
        service_manager: Option<&str>,
        launcher: &dyn crate::tools::host::daemon_lifecycle::RestartSafeHandoffLauncher,
    ) -> ApiResponse {
        let operations = match self.system_lifecycle_operations() {
            Ok(operations) => operations,
            Err(error) => return error_response(error),
        };
        match operations.queue_with(
            action,
            BackgroundDaemonConfig {
                port: self.status.port,
                ..Default::default()
            },
            executable,
            service_manager,
            launcher,
        ) {
            Ok(operation) => ApiResponse::json(202, json!({"operation": operation})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_daemon_lifecycle_operation(
        &self,
        operation_id: &str,
    ) -> ApiResponse {
        let operations = match self.system_lifecycle_operations() {
            Ok(operations) => operations,
            Err(error) => return error_response(error),
        };
        match operations.load(self.status.port, operation_id) {
            Ok(operation) => ApiResponse::json(200, json!({"operation": operation})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_source_status(&self, fetch: bool) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("inspect source promotion status");
        }
        let checkout = match self.product_paths().and_then(|paths| {
            paths.validate_source_checkout()?;
            Ok(paths.checkout.clone())
        }) {
            Ok(checkout) => checkout,
            // A checkout that cannot take source updates — a development run
            // without a deployed binary — is a normal state, not a request
            // error. The UI probes this passively on page load, so the status
            // read answers with the unavailable affordance; an explicit check
            // still surfaces the underlying error.
            Err(error) if !fetch => {
                return ApiResponse::json(
                    200,
                    json!({
                        "available": false,
                        "detail": error.to_string(),
                        "source": {"operation": null},
                        "source_check": {"freshness": "unknown", "in_flight": false},
                        "source_update": {
                            "visible": false,
                            "enabled": false,
                            "state": "unavailable",
                            "update_available": false,
                            "title": "Refine source updates are unavailable for this checkout"
                        }
                    }),
                );
            }
            Err(error) => return error_response(error),
        };
        self.handle_source_status_for_checkout(fetch, checkout)
    }

    pub(in crate::surfaces::web_server) fn handle_source_status_for_checkout(
        &self,
        fetch: bool,
        checkout: std::path::PathBuf,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("inspect source promotion status");
        };
        let service = FileSourcePromotionService::new(checkout, runtime_root, self.status.port);
        match service.request_update_check(fetch) {
            Ok(status) => ApiResponse::json(200, self.source_status_body(status)),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_source_promote(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("promote source checkout");
        };
        let checkout = match self.product_paths().and_then(|paths| {
            paths.validate_source_checkout()?;
            Ok(paths.checkout.clone())
        }) {
            Ok(checkout) => checkout,
            Err(error) => return error_response(error),
        };
        let service = FileSourcePromotionService::new(checkout, runtime_root, self.status.port);
        let refine_dir = match self.current_refine_dir() {
            Ok(Some(refine_dir)) => refine_dir,
            Ok(None) => self
                .app_registry_root
                .clone()
                .unwrap_or_else(|| runtime_root.clone()),
            Err(error) => return error_response(error),
        };
        let active_root = match self.current_target_root() {
            Ok(root) => root,
            Err(error) => return error_response(error),
        };
        let provider =
            crate::surfaces::web_server::project_routes::configured_provider_from_settings(
                &refine_dir,
                active_root.as_deref(),
                &json!({}),
            );
        match service.queue_agent(&provider) {
            Ok(operation) => ApiResponse::json(202, json!({"operation": operation})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn source_status_body(
        &self,
        status: crate::tools::host::source_promotion::CachedSourcePromotionStatus,
    ) -> serde_json::Value {
        let source = status.source;
        let check = status.check;
        let target_app_is_refine = self
            .current_target_root()
            .ok()
            .flatten()
            .as_deref()
            .is_some_and(is_refine_checkout);
        let mut source_update = source_promotion_affordance(true, &source);
        let active_operation = source
            .operation
            .as_ref()
            .is_some_and(|operation| matches!(operation.status.as_str(), "queued" | "running"));
        if !active_operation && check.in_flight {
            source_update.enabled = false;
            source_update.state = "checking".to_string();
            source_update.title = "Checking the configured Refine upstream".to_string();
        } else if !active_operation {
            if let Some(operation) = source
                .operation
                .as_ref()
                .filter(|operation| matches!(operation.status.as_str(), "failed" | "interrupted"))
            {
                source_update.enabled = source.fast_forward && source.update_available;
                source_update.state = operation.status.clone();
                source_update.title = match operation.error.as_deref() {
                    Some(error) => format!("{}: {error}", operation.message),
                    None => operation.message.clone(),
                };
            } else if let Some(failure) = check.failure.as_ref() {
                source_update.enabled = true;
                source_update.state = "error".to_string();
                source_update.title = failure.clone();
            } else if check.freshness != "fresh" {
                source_update.enabled = true;
                source_update.state = "stale".to_string();
                source_update.title = match check.last_successful_check_at.as_deref() {
                    Some(checked) => {
                        format!("Update information is stale; last successful check was {checked}")
                    }
                    None => "No remote update check has completed yet".to_string(),
                };
            }
        }
        json!({
            "source": source,
            "source_check": check,
            "source_update": source_update,
            "target_app_is_refine": target_app_is_refine
        })
    }

    pub(in crate::surfaces::web_server) fn handle_install_rollback(&self) -> ApiResponse {
        let service = match self.system_installation(env!("CARGO_PKG_VERSION")) {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        match service.rollback() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install_uninstall(&self) -> ApiResponse {
        let service = match self.system_installation(env!("CARGO_PKG_VERSION")) {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        match service.uninstall() {
            Ok(()) => ApiResponse::json(200, json!({"uninstalled": true})),
            Err(error) => error_response(error),
        }
    }
}
