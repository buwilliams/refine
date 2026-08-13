use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_goal_transition(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "mutate work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/transition"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Goal transition route requires a Goal id"
                    }
                }),
            );
        };
        let Some(status) = request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
            .and_then(GoalStatus::parse_wire)
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_status",
                        "message": "body.status must be a valid Goal status"
                    }
                }),
            );
        };

        match self
            .work_item_service(refine_dir)
            .transition_goal_status(goal_id, status)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_action(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "mutate work items");
        let Some((goal_id, action)) = goal_id_and_action(&request.path) else {
            return goal_id_required();
        };
        let service = self.work_item_service(refine_dir);
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("approve reviewed Goals"),
            Err(error) => return error_response(error),
        };
        let result = match action {
            "start" => service.start_goal_workflow(goal_id),
            "approve" => {
                let Some(runtime_root) = &self.runtime_root else {
                    return runtime_root_unavailable("approve reviewed Goals");
                };
                FileGovernanceIntegrationService::with_target_root(
                    runtime_root,
                    &service.refine_dir,
                    &target_root,
                )
                .approve_reviewed_goal(goal_id)
            }
            "verify" => match service.show_goal_summary(goal_id) {
                Ok(goal) if goal.goal.status == GoalStatus::Review => {
                    let Some(runtime_root) = &self.runtime_root else {
                        return runtime_root_unavailable("approve reviewed Goals");
                    };
                    FileGovernanceIntegrationService::with_target_root(
                        runtime_root,
                        &service.refine_dir,
                        &target_root,
                    )
                    .approve_reviewed_goal(goal_id)
                }
                Ok(_) => Err(RefineError::InvalidInput(
                    "Goal approval is only available from review".to_string(),
                )),
                Err(error) => Err(error),
            },
            "retry-quality" => service.retry_goal_quality_summary(goal_id),
            "retry-governance" => service.retry_goal_governance_summary(goal_id),
            "undo" => service.undo_goal_summary(goal_id),
            _ => {
                return ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": "unknown Goal action"
                        }
                    }),
                );
            }
        };
        match result {
            Ok(goal) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "message": goal_action_message(action),
                    "goal": goal.goal
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        match self
            .work_item_service(refine_dir)
            .delete_goal_record(goal_id)
        {
            Ok(()) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": goal_id})),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_cancel(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "cancel work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("cancel Goals");
        };
        match FileProcessControlService::with_refine_dir(runtime_root, refine_dir)
            .cancel_goal(goal_id)
        {
            Ok(result) => ApiResponse::json(200, result),
            Err(error) => error_response(error),
        }
    }
}
