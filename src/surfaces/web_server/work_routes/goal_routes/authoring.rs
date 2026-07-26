use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_goal_create(&self, request: ApiRequest) -> ApiResponse {
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("create work items"),
            Err(error) => return error_response(error),
        };
        if !target_root.exists() {
            return self.handle_goal_create_locked(request);
        }
        match with_repository_git_lock(&target_root, || Ok(self.handle_goal_create_locked(request)))
        {
            Ok(response) => response,
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_create_locked(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create work items");
        let snapshot = match self.current_projection() {
            Ok(snapshot) => snapshot,
            Err(error) => return error_response(error),
        };
        let body = request.body.as_ref();
        let service = self.work_item_service(refine_dir);
        let field = |name| {
            body.and_then(|body| body.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let result = match service.author_goal_from_projection(
            GoalAuthoringRequest {
                id: field("id"),
                name: field("name"),
                prompt: field("prompt").unwrap_or_default(),
                reporter: field("reporter").unwrap_or_default(),
                assignee: field("assignee"),
                priority: field("priority").unwrap_or_else(|| "low".to_string()),
                feature_id: field("feature_id"),
                duplicate_decision: field("duplicate_decision").unwrap_or_default(),
                ..GoalAuthoringRequest::default()
            },
            &snapshot,
        ) {
            Ok(result) => result,
            Err(RefineError::InvalidInput(message)) if message == "Goal name is required" => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_name",
                            "message": "body.name or body.prompt is required"
                        }
                    }),
                );
            }
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
        if !result.created {
            let projection_changed = result
                .move_result
                .as_ref()
                .is_some_and(|move_result| move_result.moved);
            let mut response = json!({
                "created": false,
                "duplicate_action": result.duplicate_action,
                "duplicate": { "match": result.duplicate }
            });
            if let Some(move_result) = result.move_result {
                let mut move_value = json!(move_result);
                if move_value.get("reason").is_some_and(Value::is_null) {
                    move_value.as_object_mut().unwrap().remove("reason");
                }
                response["move"] = move_value;
            }
            if projection_changed && let Err(error) = self.refresh_projection_cache_after_mutation()
            {
                return error_response(error);
            }
            return ApiResponse::json(200, response);
        }
        let goal_id = result
            .goal
            .as_ref()
            .map(|goal| goal.id.as_str())
            .unwrap_or("");
        if let Some(runtime_root) = &self.runtime_root
            && let Err(error) = BacklogPromotionService::new(&service.refine_dir, runtime_root)
                .promote_backlog_to_todo_from_projection(&snapshot, result.goal.as_ref())
        {
            return error_response(error);
        }
        if self.runtime_root.is_some() {
            let projection = match self.rebuild_current_projection_cache() {
                Ok(projection) => projection,
                Err(error) => return error_response(error),
            };
            let Some(goal) = projection.goals.get(goal_id) else {
                return error_response(RefineError::NotFound(format!(
                    "Goal {goal_id} disappeared after creation"
                )));
            };
            ApiResponse::json(201, json!({"goal": goal.goal}))
        } else {
            ApiResponse::json(201, json!({"goal": result.goal}))
        }
    }

    pub(crate) fn handle_goal_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str());
        let priority = request
            .body
            .as_ref()
            .and_then(|body| body.get("priority"))
            .and_then(|priority| priority.as_str());
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|assignee| assignee.as_str());
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|reporter| reporter.as_str());
        let notes = match request.body.as_ref().and_then(|body| body.get("notes")) {
            Some(Value::Array(notes)) => Some(notes.clone()),
            Some(_) => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_notes",
                            "message": "body.notes must be an array"
                        }
                    }),
                );
            }
            None => None,
        };
        let status = match request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
        {
            Some(status) => match GoalStatus::parse_wire(status) {
                Some(status) => Some(status),
                None => {
                    return ApiResponse::json(
                        400,
                        json!({
                            "error": {
                                "code": "invalid_status",
                                "message": "body.status must be a valid Goal status"
                            }
                        }),
                    );
                }
            },
            None => None,
        };
        let service = self.work_item_service(refine_dir);
        let mut goal = match status {
            Some(status) => match service.transition_goal_status(goal_id, status) {
                Ok(goal) => goal,
                Err(error) => return error_response(error),
            },
            None => match service.show_goal_summary(goal_id) {
                Ok(goal) => goal,
                Err(error) => return error_response(error),
            },
        };
        if name.is_some() || priority.is_some() || reporter.is_some() {
            match service.update_goal_metadata_summary(goal_id, name, priority, reporter, None) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(assignee) = assignee {
            match service.update_goal_assignee_summary(goal_id, assignee) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(notes) = notes {
            match service.replace_goal_notes_summary(goal_id, &notes) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        ApiResponse::json(200, json!({"goal": goal.goal}))
    }

    pub(crate) fn handle_goal_note(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "edit work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/notes"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let Some(body) = request
            .body
            .as_ref()
            .and_then(|body| body.get("body"))
            .and_then(|body| body.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_note",
                        "message": "body.body is required"
                    }
                }),
            );
        };
        let author = request
            .body
            .as_ref()
            .and_then(|body| body.get("author"))
            .and_then(|author| author.as_str())
            .unwrap_or("");
        match self
            .work_item_service(refine_dir)
            .add_goal_note_summary(goal_id, author, body)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }
}
