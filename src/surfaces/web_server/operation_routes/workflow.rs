use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_workflow_execution_retry(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
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
        let automation = match self.current_target_root() {
            Ok(Some(target_root)) => WorkflowEngine::with_target_root(runtime_root, target_root),
            Ok(None) => WorkflowEngine::new(runtime_root),
            Err(error) => return error_response(error),
        };
        workflow_retry_response(&automation, execution_id)
    }

    pub(in crate::surfaces::web_server) fn handle_workflow_execution_cancel(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
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
        let automation = match self.current_target_root() {
            Ok(Some(target_root)) => WorkflowEngine::with_target_root(runtime_root, target_root),
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
}
