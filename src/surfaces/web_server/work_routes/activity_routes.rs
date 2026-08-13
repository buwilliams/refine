use super::{
    ActivityProjectionQuery, ActivityService, ApiRequest, ApiResponse, ChangeProjectionQuery,
    FileActivityService, FileGitWorktreeService, FileMetricsService, FileOperationRegistry,
    GitWorktreeService, GoalStatus, InProcessWebServer, OperationRegistry, OperationState,
    PROJECTION_SNAPSHOT_FILE, PageRequest, PerformanceQuery, ProjectionQuery, Value,
    bounded_query_usize, error_response, json, operation_response, performance_report_value,
    query_param, runtime_root_unavailable, target_root_unavailable, thread,
    with_repository_git_lock,
};

impl InProcessWebServer {
    pub(crate) fn handle_activity_list(&self, raw_path: &str) -> ApiResponse {
        let Some(_) = (match self.current_refine_dir() {
            Ok(refine_dir) => refine_dir,
            Err(error) => return error_response(error),
        }) else {
            return target_root_unavailable("read activity");
        };
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        let result = projection.list_activity(ActivityProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "datetime".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            goal_id: query_param(raw_path, "goal_id"),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
            q: query_param(raw_path, "q"),
        });
        ApiResponse::json(
            200,
            json!({
                "activity": result.activity,
                "facets": result.facets,
                "matching_ids": result.matching_ids,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset + limit < result.total,
                    "total": result.total
                }
            }),
        )
    }

    pub(crate) fn handle_activity_ui_error(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "record UI activity");
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("UI error")
            .trim();
        let service = FileActivityService::new(refine_dir);
        let mut entry = service.new_entry(
            if message.is_empty() {
                "UI error"
            } else {
                message
            },
            "error",
            "ui",
            body.get("goal_id")
                .and_then(|goal_id| goal_id.as_str())
                .map(str::to_string),
            Some("browser".to_string()),
        );
        if let Some(details) = body.as_object() {
            entry.details = Some(details.clone());
        }
        match service.append(entry.clone()) {
            Ok(()) => ApiResponse::json(200, json!({"recorded": true, "entry": entry})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_activity_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "clean up activity");
        let body = request.body.unwrap_or_else(|| json!({}));
        let days = body
            .get("days")
            .and_then(|value| value.as_i64())
            .unwrap_or(7);
        let clear = body
            .get("clear")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || days == 0;
        let service = FileActivityService::new(refine_dir);
        match service.cleanup(days, clear) {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": days
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_changes_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        let result = projection.list_changes(ChangeProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "committed".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            goal_id: query_param(raw_path, "goal_id"),
            status: query_param(raw_path, "status")
                .and_then(|status| GoalStatus::parse_wire(&status)),
            priority: query_param(raw_path, "priority"),
            branch: query_param(raw_path, "branch"),
        });
        let branch = result
            .changes
            .iter()
            .find_map(|change| change.branch.clone())
            .or_else(|| {
                self.target_root().and_then(|target_root| {
                    FileGitWorktreeService::new(target_root)
                        .inspect("")
                        .ok()
                        .and_then(|status| status.branch)
                })
            });
        let changes = result
            .changes
            .iter()
            .map(|change| {
                json!({
                    "commit": change.commit,
                    "goal_id": change.goal_id,
                    "name": change.goal_name,
                    "status": change.goal_status,
                    "priority": change.goal_priority,
                    "assignee": change.goal_assignee,
                    "committed": change.committed_time,
                    "subject": change.subject,
                    "branch": change.branch
                })
            })
            .collect::<Vec<_>>();
        ApiResponse::json(
            200,
            json!({
                "branch": branch,
                "changes": changes,
                "matching_ids": result.matching_ids,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset + limit < result.total,
                    "total": result.total
                }
            }),
        )
    }

    pub(crate) fn handle_changes_undo(&self, request: ApiRequest) -> ApiResponse {
        let commit = request
            .body
            .as_ref()
            .and_then(|body| body.get("commit"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if commit.is_empty() {
            return ApiResponse::json(
                400,
                json!({
                    "ok": false,
                    "error": {
                        "code": "invalid_input",
                        "message": "body.commit is required"
                    }
                }),
            );
        }
        let refine_dir = require_refine_dir!(self, "undo Git changes");
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("undo Git changes");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("undo Git changes");
        };
        let linked_goal_id = self.current_projection().ok().and_then(|projection| {
            projection
                .changes
                .values()
                .find(|change| change.commit == commit)
                .and_then(|change| change.goal_id.clone())
        });
        let registry = FileOperationRegistry::new(runtime_root);
        let operation_owner = format!("changes:undo:{commit}");
        let operation = match registry.register(&operation_owner) {
            Ok(operation) => operation,
            Err(error) => return error_response(error),
        };
        let _ = registry.update_progress(
            &operation.id,
            json!({"message": "Undoing Git change", "commit": commit}),
        );
        let operation = registry.status(&operation.id).unwrap_or(operation);
        let operation_id = operation.id.clone();
        let runtime_root = runtime_root.clone();
        let commit = commit.to_string();
        let server = self.clone();
        thread::spawn(move || {
            let registry = FileOperationRegistry::new(&runtime_root);
            let result = with_repository_git_lock(&target_root, || {
                FileGitWorktreeService::with_runtime_root(&target_root, &runtime_root)
                    .revert_commit(&commit)
            });
            match result {
                Ok(result) => {
                    let cancelled_goal = if result.ok {
                        match linked_goal_id.as_deref() {
                            Some(goal_id) => match server
                                .work_item_service(refine_dir)
                                .cancel_goal_summary(goal_id)
                            {
                                Ok(goal) => Some(goal.goal.id),
                                Err(error) => {
                                    let _ = registry.fail_with_error(
                                        &operation_id,
                                        json!({
                                            "code": "goal_cancellation_failed",
                                            "message": error.to_string()
                                        }),
                                    );
                                    return;
                                }
                            },
                            None => None,
                        }
                    } else {
                        None
                    };
                    if let Err(error) = server.rebuild_current_projection_cache() {
                        let _ = registry.fail_with_error(
                            &operation_id,
                            json!({
                                "code": "projection_refresh_failed",
                                "message": error.to_string()
                            }),
                        );
                        return;
                    }
                    let response = json!({
                        "ok": result.ok,
                        "pushed": false,
                        "commit": commit,
                        "conflicts": result.conflicts,
                        "message": result.message.unwrap_or_default(),
                        "goal_id": linked_goal_id,
                        "cancelled_goal": cancelled_goal
                    });
                    let _ = registry.finish_with_result(
                        &operation_id,
                        OperationState::Succeeded,
                        response,
                    );
                }
                Err(error) => {
                    let _ = registry.fail_with_error(
                        &operation_id,
                        json!({
                            "code": "git_undo_failed",
                            "message": error.to_string()
                        }),
                    );
                }
            }
        });
        ApiResponse::json(202, json!({"operation": operation_response(operation)}))
    }

    pub(crate) fn handle_cache_rebuild(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rebuild projection cache");
        };
        if request
            .body
            .as_ref()
            .and_then(|body| body.get("background"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("cache:rebuild") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let _ = registry.update_progress(
                &operation.id,
                json!({"message": "Rebuilding projection cache"}),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let operation_id = operation.id.clone();
            let runtime_root = runtime_root.clone();
            let server = self.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                match server.rebuild_current_projection_cache() {
                    Ok(projection) => {
                        let result = json!({
                            "ok": true,
                            "mode": "rebuilt",
                            "goals": projection.goals.len(),
                            "features": projection.features.len(),
                            "projection_version": projection.version,
                            "cache": runtime_root
                                .join("cache")
                                .join(PROJECTION_SNAPSHOT_FILE)
                                .display()
                                .to_string()
                        });
                        let _ = registry.update_progress(
                            &operation_id,
                            json!({"message": "Projection cache rebuilt"}),
                        );
                        let _ = registry.finish_with_result(
                            &operation_id,
                            OperationState::Succeeded,
                            result,
                        );
                    }
                    Err(error) => {
                        let _ = registry.fail_with_error(
                            &operation_id,
                            json!({
                                "code": "projection_rebuild_failed",
                                "message": error.to_string()
                            }),
                        );
                    }
                }
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
        let projection = match self.rebuild_current_projection_cache() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let cache_dir = runtime_root.join("cache");
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "mode": "rebuilt",
                "goals": projection.goals.len(),
                "features": projection.features.len(),
                "projection_version": projection.version,
                "cache": cache_dir.join(PROJECTION_SNAPSHOT_FILE).display().to_string()
            }),
        )
    }

    pub(crate) fn handle_performance_list(&self, raw_path: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read performance metrics");
        };
        let query = PerformanceQuery {
            limit: bounded_query_usize(raw_path, "limit", 50, 1000),
            offset: bounded_query_usize(raw_path, "offset", 0, usize::MAX),
            operation: query_param(raw_path, "operation").filter(|value| !value.is_empty()),
            success: query_param(raw_path, "success").and_then(|value| match value.as_str() {
                "1" | "true" | "True" | "TRUE" => Some(true),
                "0" | "false" | "False" | "FALSE" => Some(false),
                _ => None,
            }),
        };
        if query == PerformanceQuery::default()
            && let Ok(runtime) = self.current_runtime_projection()
            && let Some(performance) = runtime.performance
        {
            return ApiResponse::json(200, serde_json::Value::Object(performance));
        }
        match performance_report_value(runtime_root, query) {
            Ok(value) => {
                let response = ApiResponse::json(200, value.clone());
                if let Some(performance) = value.as_object().cloned() {
                    let _ = self.persist_runtime_projection_override(|runtime| {
                        runtime.performance = Some(performance);
                    });
                }
                response
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_performance_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("clean up performance metrics");
        };
        let clear = request
            .body
            .as_ref()
            .and_then(|body| body.get("clear"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let service = FileMetricsService::new(runtime_root);
        match service.cleanup(clear) {
            Ok(result) => {
                let _ = performance_report_value(runtime_root, PerformanceQuery::default())
                    .and_then(|value| {
                        if let Some(performance) = value.as_object().cloned() {
                            self.persist_runtime_projection_override(|runtime| {
                                runtime.performance = Some(performance);
                            })?;
                        }
                        Ok(())
                    });
                ApiResponse::json(
                    200,
                    json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": service.retention_days
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_governance_integration_hard_reset_worktree(&self) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("hard-reset Git worktree");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("hard-reset Git worktree");
        };
        let registry = FileOperationRegistry::new(runtime_root);
        let operation = match registry.register_exclusive_with_request(
            "changes:hard-reset",
            json!({"target_root": target_root}),
        ) {
            Ok(operation) => operation,
            Err(error) => return error_response(error),
        };
        let _ = registry.update_progress(
            &operation.id,
            json!({"message": "Hard-resetting target worktree"}),
        );
        let operation = registry.status(&operation.id).unwrap_or(operation);
        let operation_id = operation.id.clone();
        let runtime_root = runtime_root.clone();
        thread::spawn(move || {
            let registry = FileOperationRegistry::new(&runtime_root);
            match with_repository_git_lock(&target_root, || {
                FileGitWorktreeService::with_runtime_root(&target_root, &runtime_root).hard_reset()
            }) {
                Ok(result) => {
                    let _ = registry.finish_with_result(
                        &operation_id,
                        OperationState::Succeeded,
                        json!({
                            "ok": result.ok,
                            "conflicts": result.conflicts,
                            "message": result.message.unwrap_or_default()
                        }),
                    );
                }
                Err(error) => {
                    let _ = registry.fail_with_error(
                        &operation_id,
                        json!({
                            "code": "git_hard_reset_failed",
                            "message": error.to_string()
                        }),
                    );
                }
            }
        });
        ApiResponse::json(202, json!({"operation": operation_response(operation)}))
    }
}
