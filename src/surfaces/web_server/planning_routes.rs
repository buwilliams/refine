use super::*;
use crate::application::planning::{FilePlanningService, PlanningCommand};
use crate::error::{RefineError, RefineResult};
use serde_json::json;
impl InProcessWebServer {
    pub(crate) fn handle_planning(&self, request: ApiRequest) -> ApiResponse {
        let result = (|| -> RefineResult<(u16, serde_json::Value)> {
            let root = self
                .current_refine_dir()?
                .ok_or_else(|| RefineError::InvalidInput("Attach a project first".into()))?;
            let target = self
                .current_target_root()?
                .ok_or_else(|| RefineError::InvalidInput("Attach a project first".into()))?;
            let runtime = self
                .runtime_root
                .as_ref()
                .ok_or_else(|| RefineError::InvalidInput("Runtime unavailable".into()))?;
            let service = FilePlanningService::new(root, target, runtime)?;
            let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
            match (request.method.as_str(), parts.as_slice()) {
                ("GET", ["planning"]) => Ok((200, service.snapshot()?)),
                ("GET", ["planning", "boards", id]) => Ok((200, json!(service.board(id)?))),
                ("GET", ["planning", "cards", id]) => Ok((200, json!(service.placement(id)?))),
                ("GET", ["planning", "actions", id]) => Ok((200, json!(service.action(id)?))),
                ("POST", ["planning", "actions", id, "cancel"]) => {
                    Ok((200, json!(service.cancel(id)?)))
                }
                ("POST", ["planning", "commands"]) => {
                    let command: PlanningCommand =
                        serde_json::from_value(request.body.unwrap_or(json!({})))
                            .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
                    let immediate = command.operation.starts_with("board.")
                        || command.operation.starts_with("lane.")
                        || command.operation == "migrate";
                    let action = service.submit(command)?;
                    if immediate {
                        service.process_action(&action.id)?;
                    }
                    let action = service.action(&action.id)?;
                    Ok((if action.terminal() { 200 } else { 202 }, json!(action)))
                }
                _ => Err(RefineError::NotFound("Planning route".into())),
            }
        })();
        match result {
            Ok((status, value)) => ApiResponse::json(status, value),
            Err(e) => super::support::error_response(e),
        }
    }
}
