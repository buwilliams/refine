use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_feature_move(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "move Feature workflow");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/move"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(target) = request
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
                        "message": "body.status must be backlog or todo"
                    }
                }),
            );
        };
        match self
            .work_item_service(refine_dir)
            .move_feature_workflow(feature_id, target)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_transfer(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer Feature to node");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/transfer"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let Some(target_node_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target_node_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_node_id",
                        "message": "body.target_node_id is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(refine_dir)
            .transfer_feature_to_node(target_node_id, feature_id)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_cancel(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "cancel Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let goal_ids = match self.current_projection_shared() {
            Ok(projection) => projection
                .features
                .get(feature_id)
                .map(|feature| feature.goal_ids.clone())
                .unwrap_or_default(),
            Err(error) => return error_response(error),
        };
        let runtime_reconciled = match self.reconcile_feature_runtime_work(feature_id, &goal_ids) {
            Ok(summary) => summary,
            Err(error) => return error_response(error),
        };
        match self
            .work_item_service(refine_dir)
            .cancel_feature_summary(feature_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup,
                    "runtime_reconciled": {
                        "processes": runtime_reconciled.processes,
                        "operations": runtime_reconciled.operations
                    }
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        match self
            .work_item_service(refine_dir)
            .delete_feature_record(feature_id)
        {
            Ok(()) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": feature_id})),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }
}
