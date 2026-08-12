use super::*;

impl InProcessWebServer {
    fn system_runtime_root(&self) -> RefineResult<std::path::PathBuf> {
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
            "HTTP system update is disabled; run `./r system update` from the Refine checkout so it can stop daemons, delegate the update to the configured agent, and restart ports.".to_string(),
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
        let checkout = match discover_refine_checkout() {
            Ok(checkout) => checkout,
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
        match service.inspect(fetch) {
            Ok(source) => ApiResponse::json(200, self.source_status_body(source)),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_source_promote(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("promote source checkout");
        };
        let checkout = match discover_refine_checkout() {
            Ok(checkout) => checkout,
            Err(error) => return error_response(error),
        };
        let service = FileSourcePromotionService::new(checkout, runtime_root, self.status.port);
        match service.queue() {
            Ok(operation) => ApiResponse::json(202, json!({"operation": operation})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn source_status_body(
        &self,
        source: SourcePromotionSnapshot,
    ) -> serde_json::Value {
        let target_app_is_refine = self
            .current_target_root()
            .ok()
            .flatten()
            .as_deref()
            .is_some_and(is_refine_checkout);
        let source_update = source_promotion_affordance(target_app_is_refine, &source);
        json!({
            "source": source,
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
