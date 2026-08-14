use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_feature_show(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let service = FileWorkItemService::new(refine_dir);
        match service.show_feature_summary(feature_id) {
            Ok(feature) => {
                let goals = feature
                    .goal_ids
                    .iter()
                    .filter_map(|goal_id| {
                        let goal = service.show_goal_summary(goal_id).ok()?;
                        let capability =
                            FileWorkItemService::feature_goal_authoring_capability(&goal);
                        let mut value = serde_json::to_value(goal.goal).ok()?;
                        value.as_object_mut()?.insert(
                            "feature_authoring".to_string(),
                            serde_json::to_value(capability).ok()?,
                        );
                        Some(value)
                    })
                    .collect::<Vec<_>>();
                let feature_detail = feature_detail_response_from_goals(&feature, goals);
                ApiResponse::json(
                    200,
                    json!({
                        "feature": feature_detail,
                        "goal_ids": feature.goal_ids,
                        "rollup": feature.rollup
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_features_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection_shared() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let page = bounded_query_usize(raw_path, "page", 1, usize::MAX).max(1);
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| (page - 1).saturating_mul(limit));
        let current_node_id = self.active_node_id_for_routes();
        let query = FeatureProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status")
                .and_then(|value| GoalStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            assignee: query_param(raw_path, "assignee"),
            node: query_param(raw_path, "node"),
            current_node_id: Some(current_node_id),
        };
        let result = projection.list_features(query);
        let features: Vec<_> = result
            .features
            .into_iter()
            .map(|feature| {
                json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                })
            })
            .collect();
        ApiResponse::json(
            200,
            json!({
                "features": features,
                "matching_ids": result.matching_ids,
                "projection_version": projection.version,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "page": page,
                    "total": result.total,
                    "has_more": offset + limit < result.total
                }
            }),
        )
    }
}
