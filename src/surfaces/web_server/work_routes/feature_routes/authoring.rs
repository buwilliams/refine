use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_feature_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create features");
        let Some(name) = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name is required"
                    }
                }),
            );
        };
        let id = request
            .body
            .as_ref()
            .and_then(|body| body.get("id"))
            .and_then(|id| id.as_str());
        let description = request
            .body
            .as_ref()
            .and_then(|body| body.get("description"))
            .and_then(|description| description.as_str());
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|reporter| reporter.as_str());
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|assignee| assignee.as_str());
        match self.work_item_service(refine_dir).create_feature_summary(
            name,
            id,
            description,
            reporter,
            assignee,
        ) {
            Ok(feature) => ApiResponse::json(
                201,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match self
            .work_item_service(refine_dir)
            .update_feature_metadata_summary(
                feature_id,
                body.get("name").and_then(|value| value.as_str()),
                body.get("description").and_then(|value| value.as_str()),
                body.get("reporter").and_then(|value| value.as_str()),
                body.get("assignee").and_then(|value| value.as_str()),
            ) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_feature_goal_author(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "author Feature Goals");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/goals/author"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let authoring = match serde_json::from_value::<FeatureGoalAuthoringRequest>(body) {
            Ok(authoring) => authoring,
            Err(error) => {
                return error_response(RefineError::InvalidInput(format!(
                    "invalid Feature Goal authoring body: {error}"
                )));
            }
        };
        let result = match self
            .work_item_service(refine_dir)
            .author_feature_goal(feature_id, authoring)
        {
            Ok(result) => result,
            Err(error) => return error_response(error),
        };
        if result.requires_duplicate_decision {
            return ApiResponse::json(
                409,
                json!({
                    "error": {
                        "code": "duplicate_goal",
                        "message": "Possible duplicate Goal",
                        "duplicate": { "match": result.duplicate }
                    }
                }),
            );
        }
        if result.created
            && let Err(error) = self.promote_backlog_after_mutation()
        {
            return error_response(error);
        }
        if let Err(error) = self.refresh_projection_cache_after_mutation() {
            return error_response(error);
        }
        ApiResponse::json(
            if result.created { 201 } else { 200 },
            json!({
                "created": result.created,
                "goal": result.goal,
                "duplicate_action": result.duplicate_action,
                "duplicate": result.duplicate.map(|duplicate| json!({"match": duplicate})),
                "move": result.move_result
            }),
        )
    }
}
