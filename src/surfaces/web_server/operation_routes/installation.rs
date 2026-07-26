use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_install_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).status() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
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

    pub(in crate::surfaces::web_server) fn handle_install_repair(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("repair install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).repair() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install_update(
        &self,
        _request: ApiRequest,
    ) -> ApiResponse {
        error_response(RefineError::NotImplemented(
            "HTTP system update is disabled; run `./r system update` from the Refine checkout so the installer can stop daemons, update the deployed binary, refresh service metadata, and restart ports.".to_string(),
        ))
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
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rollback install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).rollback() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_install_uninstall(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("uninstall Refine");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).uninstall() {
            Ok(()) => ApiResponse::json(200, json!({"uninstalled": true})),
            Err(error) => error_response(error),
        }
    }
}
