use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_goal_round_append(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "append Goal rounds");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/rounds"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let Some(reporter) = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let Some(prompt) = request
            .body
            .as_ref()
            .and_then(|body| body.get("prompt"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|value| value.as_str());
        match self
            .work_item_service(refine_dir)
            .append_goal_round_summary_with_assignee(goal_id, reporter, assignee, prompt)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_round_edit_latest(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "edit latest Goal round");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/rounds/latest"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str());
        let prompt = request
            .body
            .as_ref()
            .and_then(|body| body.get("prompt"))
            .and_then(|value| value.as_str());
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|value| value.as_str());
        match self
            .work_item_service(refine_dir)
            .edit_latest_goal_round_summary(goal_id, reporter, assignee, prompt)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_round_evaluation_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update latest Goal round evaluation");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/rounds/latest/evaluation"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match self
            .work_item_service(refine_dir)
            .update_latest_goal_round_evaluation_summary(goal_id, &body)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_round_log_append(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "append Goal round logs");
        let Some(rest) = request.path.strip_prefix("/work/goals/") else {
            return goal_id_required();
        };
        let Some((goal_id, round_part)) = rest.split_once("/rounds/") else {
            return goal_id_required();
        };
        let Some(round_idx) = round_part
            .strip_suffix("/logs")
            .and_then(|value| value.parse::<usize>().ok())
        else {
            return ApiResponse::json(
                400,
                json!({"error": {"code": "invalid_round", "message": "round index is required"}}),
            );
        };
        let goal = match self
            .work_item_service(&refine_dir)
            .show_goal_summary(goal_id)
        {
            Ok(goal) => goal,
            Err(error) => return error_response(error),
        };
        if round_idx >= goal.goal.round_count {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return ApiResponse::json(
                400,
                json!({"error": {"code": "invalid_log", "message": "log message is required"}}),
            );
        }
        let entry = LogEntry {
            datetime: body
                .get("datetime")
                .and_then(|datetime| datetime.as_str())
                .unwrap_or("")
                .to_string(),
            severity: body
                .get("severity")
                .and_then(|severity| severity.as_str())
                .unwrap_or("info")
                .to_string(),
            category: body
                .get("category")
                .and_then(|category| category.as_str())
                .unwrap_or("state")
                .to_string(),
            message: message.to_string(),
            details: body
                .get("details")
                .and_then(|details| details.as_object())
                .cloned(),
            actions: Vec::new(),
            actor: body
                .get("actor")
                .and_then(|actor| actor.as_str())
                .map(str::to_string),
            goal_id: Some(goal_id.to_string()),
        };
        match FileLogService::new(refine_dir).append_round_log(goal_id, round_idx, entry) {
            Ok(log) => ApiResponse::json(
                200,
                json!({"log": log, "goal_id": goal_id, "round_idx": round_idx}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_logs(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Goal round logs");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        let goal = match self
            .work_item_service(&refine_dir)
            .show_goal_summary(goal_id)
        {
            Ok(goal) => goal,
            Err(error) => return error_response(error),
        };
        if goal.goal.round_count == 0 {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let round_idx = 0;
        match FileLogService::new(refine_dir).page_round_logs(goal_id, round_idx, 50, 0) {
            Ok((logs, has_more, total)) => ApiResponse::json(
                200,
                json!({
                    "goal_id": goal_id,
                    "round_idx": round_idx,
                    "logs": logs,
                    "pagination": {
                        "limit": 50,
                        "offset": 0,
                        "total": total,
                        "has_more": has_more
                    },
                    "round_log_count": total,
                    "activity_count": 0
                }),
            ),
            Err(error) => error_response(error),
        }
    }
}
