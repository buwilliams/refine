use super::*;

impl InProcessWebServer {
    fn release_service(&self, action: &str) -> Result<FileReleaseService, ApiResponse> {
        let Some(runtime_root) = &self.runtime_root else {
            return Err(runtime_root_unavailable(action));
        };
        let repo_root = match self.current_target_root() {
            Ok(Some(root)) => root,
            Ok(None) => return Err(target_root_unavailable(action)),
            Err(error) => return Err(error_response(error)),
        };
        Ok(FileReleaseService::new(repo_root, runtime_root))
    }

    pub(in crate::surfaces::web_server) fn handle_releases_status(&self) -> ApiResponse {
        let service = match self.release_service("inspect releases") {
            Ok(service) => service,
            Err(response) => return response,
        };
        match service.status() {
            Ok(releases) => ApiResponse::json(200, json!({"releases": releases})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_release_plan(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let service = match self.release_service("plan a release") {
            Ok(service) => service,
            Err(response) => return response,
        };
        let bump = request
            .body
            .as_ref()
            .and_then(|body| body.get("bump"))
            .and_then(Value::as_str)
            .unwrap_or("");
        match ReleaseBump::parse(bump).and_then(|bump| service.plan(bump)) {
            Ok(plan) => ApiResponse::json(200, json!({"plan": plan})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_release_prepare(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let service = match self.release_service("prepare a release") {
            Ok(service) => service,
            Err(response) => return response,
        };
        let bump = request
            .body
            .as_ref()
            .and_then(|body| body.get("bump"))
            .and_then(Value::as_str)
            .unwrap_or("");
        match ReleaseBump::parse(bump).and_then(|bump| service.start_prepare(bump)) {
            Ok(operation) => {
                ApiResponse::json(202, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_release_publish(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let service = match self.release_service("publish a release") {
            Ok(service) => service,
            Err(response) => return response,
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let confirmed = body
            .get("confirmed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let preparation_id = body
            .get("preparation_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RefineError::InvalidInput("release preparation_id is required".to_string())
            });
        match preparation_id
            .and_then(|preparation_id| service.start_publish(preparation_id, confirmed))
        {
            Ok(operation) => {
                ApiResponse::json(202, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_release_retry(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let service = match self.release_service("retry a release operation") {
            Ok(service) => service,
            Err(response) => return response,
        };
        let Some(operation_id) = request
            .path
            .strip_prefix("/system/releases/")
            .and_then(|path| path.strip_suffix("/retry"))
            .filter(|id| !id.is_empty() && !id.contains('/'))
        else {
            return operation_id_required();
        };
        let confirmed = request
            .body
            .as_ref()
            .and_then(|body| body.get("confirmed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match service.retry(operation_id, confirmed) {
            Ok(operation) => {
                ApiResponse::json(202, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }
}
