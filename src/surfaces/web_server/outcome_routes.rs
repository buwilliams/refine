//! Thin workflow outcome adapter; no surface-specific transition semantics.
use super::support::*;
use super::*;
use crate::application::work_items::WorkflowControl;
use serde_json::{Value, json};
impl InProcessWebServer {
    pub(super) fn handle_workflow_control(&self, request: ApiRequest) -> ApiResponse {
        let result = (|| -> crate::error::RefineResult<Value> {
            let root = self.current_refine_dir()?.ok_or_else(|| {
                crate::error::RefineError::InvalidInput("Attach a target app first".into())
            })?;
            let service = self.work_item_service(&root);
            let rest = request
                .path
                .strip_prefix("/workflow/goals/")
                .unwrap_or_default();
            let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
            if id.is_empty() || id.contains('/') {
                return Err(crate::error::RefineError::InvalidInput(
                    "Goal id required".into(),
                ));
            }
            if request.method == "GET" && action.is_empty() {
                return service.show_goal_detail(id);
            }
            if request.method != "POST" {
                return Err(crate::error::RefineError::InvalidInput(
                    "Workflow controls require POST".into(),
                ));
            }
            let body = request.body.unwrap_or(json!({}));
            let control: WorkflowControl = serde_json::from_value(body)
                .map_err(|e| crate::error::RefineError::InvalidInput(e.to_string()))?;
            match action {
                "move" => service.control_workflow(id, &control),
                "integrate" => {
                    let runtime = self.runtime_root.as_ref().ok_or_else(|| {
                        crate::error::RefineError::InvalidInput("Runtime unavailable".into())
                    })?;
                    crate::application::workflow::governance::integration::FileGovernanceIntegrationService::with_target_root(runtime,&root,self.current_target_root()?.ok_or_else(||crate::error::RefineError::InvalidInput("Target unavailable".into()))?).force_integrate(id,&control)
                }
                _ => Err(crate::error::RefineError::NotFound(
                    "Workflow control not found".into(),
                )),
            }
        })();
        match result {
            Ok(v) => ApiResponse::json(200, v),
            Err(e) => error_response(e),
        }
    }
}
