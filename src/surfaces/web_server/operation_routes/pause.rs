use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_workflow_pause(
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

    pub(in crate::surfaces::web_server) fn set_workflow_paused_response(
        &self,
        paused: bool,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control workflow automation");
        };
        let target_root = match self.current_target_root() {
            Ok(root) => root,
            Err(error) => return error_response(error),
        };
        let result = if let Some(target_root) = target_root {
            WorkflowEngine::with_target_root(runtime_root, target_root).set_workflow_paused(paused)
        } else {
            FileProcessSupervisor::new(runtime_root).set_workflow_paused(paused)
        };
        match result {
            Ok(_) => self.handle_processes("/processes"),
            Err(error) => error_response(error),
        }
    }
}
