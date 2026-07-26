use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_feature_add_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "assign Goals to Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/goals"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(goal_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("goal_id"))
            .and_then(|goal_id| goal_id.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_goal_id",
                        "message": "body.goal_id is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(refine_dir)
            .assign_goal_to_feature(feature_id, goal_id)
        {
            Ok(feature) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(
                    200,
                    json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
                ),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_add_goal_path(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "assign Goals to Features");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let goal_id = goal_part;
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .assign_goal_to_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_remove_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "remove Goals from Features");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let goal_id = goal_part;
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .remove_goal_from_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_order_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "order Feature Goals");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let Some(goal_id) = goal_part.strip_suffix("/order") else {
            return goal_id_required();
        };
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .order_goal_in_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_unorder_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "unorder Feature Goals");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let Some(goal_id) = goal_part.strip_suffix("/unorder") else {
            return goal_id_required();
        };
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .unorder_goal_in_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_reorder_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "reorder Feature Goals");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let Some(goal_id) = goal_part.strip_suffix("/reorder") else {
            return goal_id_required();
        };
        let order = match self
            .current_projection()
            .map_err(error_response)
            .and_then(|projection| {
                feature_reorder_order_from_body(
                    request.body.as_ref(),
                    &projection,
                    feature_id,
                    goal_id,
                )
            }) {
            Ok(order) => order,
            Err(response) => return response,
        };
        match self
            .work_item_service(refine_dir)
            .reorder_goal_in_feature(feature_id, goal_id, order)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }
}
