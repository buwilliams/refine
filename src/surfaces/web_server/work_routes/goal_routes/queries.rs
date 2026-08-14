use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_goal_bulk_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk update work items");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some(update) = parse_bulk_goal_update(body) else {
            return invalid_bulk_body();
        };
        if matches!(
            &update,
            BulkGoalUpdate::Status(status) if status.trim().eq_ignore_ascii_case("cancelled")
        ) {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("cancel Goals");
            };
            return match FileProcessControlService::with_refine_dir(runtime_root, &refine_dir)
                .bulk_cancel_goals(selection)
            {
                Ok(result) => ApiResponse::json(200, json!(result)),
                Err(error) => error_response(error),
            };
        }
        match self
            .work_item_service(refine_dir)
            .bulk_update_goals(selection, update)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_bulk_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk delete work items");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_delete_goals(selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_show(&self, request: ApiRequest) -> ApiResponse {
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Goal route requires a Goal id"
                    }
                }),
            );
        };
        let refine_dir = require_refine_dir!(self, "read Goal detail");
        match self.work_item_service(refine_dir).show_goal_detail(goal_id) {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goals_list(&self, raw_path: &str) -> ApiResponse {
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
        let query = GoalProjectionQuery {
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
            feature: query_param(raw_path, "feature"),
            rounds_gte: query_param(raw_path, "rounds_gte")
                .and_then(|value| value.parse::<usize>().ok()),
            rounds_lte: query_param(raw_path, "rounds_lte")
                .and_then(|value| value.parse::<usize>().ok()),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
        };
        let include_facets = query_param(raw_path, "facets").is_some_and(|value| value == "1");
        let mut facet_query = query.clone();
        facet_query.status = None;
        facet_query.page.offset = 0;
        facet_query.page.limit = 0;
        let facet_status_counts =
            include_facets.then(|| projection.list_goals(facet_query).filtered_status_counts);
        let activity_facets = include_facets.then(|| {
            projection
                .list_activity(ActivityProjectionQuery::default())
                .facets
        });
        let result = projection.list_goals(query);
        let node_identities = self.node_identities_for_routes();
        let goals = result
            .goals
            .into_iter()
            .map(|goal| {
                let node_id = goal.node_id.as_deref().unwrap_or("default");
                let fallback_display_name = if node_id == "default" {
                    "Default"
                } else {
                    node_id
                };
                let node_identity = node_identities.get(node_id).cloned();
                let mut value = json!(goal);
                if let Some(identity) = node_identity
                    && let Some(object) = value.as_object_mut()
                {
                    object.insert(
                        "node_display_name".to_string(),
                        json!(identity.display_name),
                    );
                    if !identity.diagnostics.is_empty() {
                        object.insert(
                            "node_identity_diagnostics".to_string(),
                            json!(identity.diagnostics),
                        );
                    }
                } else if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "node_display_name".to_string(),
                        json!(fallback_display_name),
                    );
                }
                value
            })
            .collect::<Vec<_>>();
        let mut body = json!({
            "goals": goals,
            "counts": projection.status_counts(),
            "filtered_counts": result.filtered_status_counts,
            "matching_ids": result.matching_ids,
            "projection_version": projection.version,
            "page": {
                "limit": limit,
                "offset": offset,
                "page": page,
                "total": result.total,
                "has_more": offset + limit < result.total
            }
        });
        if let Some(status_counts) = facet_status_counts {
            body["facets"] = json!({
                "status_counts": status_counts,
                "categories": activity_facets
                    .as_ref()
                    .map(|facets| facets.categories.clone())
                    .unwrap_or_default(),
                "severities": activity_facets
                    .as_ref()
                    .map(|facets| facets.severities.clone())
                    .unwrap_or_default(),
                "actors": activity_facets
                    .as_ref()
                    .map(|facets| facets.actors.clone())
                    .unwrap_or_default()
            });
        }
        ApiResponse::json(200, body)
    }
}
