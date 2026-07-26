use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_feature_bulk_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk update features");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkFeatureSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some((field, value)) =
            body.get("update")
                .and_then(Value::as_object)
                .and_then(|update| {
                    if update.len() == 1 {
                        update.iter().next()
                    } else {
                        None
                    }
                })
        else {
            return invalid_bulk_body();
        };
        let Some(value) = value.as_str() else {
            return invalid_bulk_body();
        };
        let update = match field.as_str() {
            "reporter" => BulkFeatureUpdate::Reporter(value.to_string()),
            "assignee" => BulkFeatureUpdate::Assignee(value.to_string()),
            _ => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_update_features(selection, update)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_bulk_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk delete features");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkFeatureSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_delete_features(selection)
        {
            Ok(result) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!(result)),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_node_transfer_features(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer Features to node");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkFeatureSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some(target_node_id) = body
            .get("target_node_id")
            .and_then(Value::as_str)
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
            .bulk_transfer_features_to_node(target_node_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_bulk_assign_goals(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk assign Goals to Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/goals/bulk"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_assign_goals_to_feature(feature_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }
}
