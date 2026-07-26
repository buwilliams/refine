use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_import_persist(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "persist imported Goals");
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("persist imported Goals in the background");
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("import:persist") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let draft_total = body
                .get("drafts")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": "Saving import",
                    "completed": 0,
                    "total": draft_total
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let server = self.clone();
            let operation_id = operation.id.clone();
            let runtime_root = runtime_root.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = server.import_persist_background_response(
                    refine_dir,
                    body,
                    &registry,
                    &operation_id,
                );
                if response.status == 499 {
                    return;
                }
                let mut result = response.body.clone();
                match result.as_object_mut() {
                    Some(object) => {
                        object.insert("http_status".to_string(), json!(response.status));
                    }
                    None => {
                        result = json!({
                            "http_status": response.status,
                            "body": result
                        });
                    }
                }
                let count = result.get("count").and_then(Value::as_u64).unwrap_or(0);
                let _ = registry.update_progress(
                    &operation_id,
                    json!({
                        "message": "Import saved",
                        "completed": count,
                        "total": draft_total
                    }),
                );
                let refresh_result = server.refresh_projection_cache_after_mutation();
                let state = if response.status >= 400 {
                    OperationState::Failed
                } else {
                    OperationState::Succeeded
                };
                if let Err(error) = refresh_result {
                    let _ = registry.fail_with_error(
                        &operation_id,
                        json!({
                            "code": "projection_refresh_failed",
                            "message": error.to_string()
                        }),
                    );
                } else {
                    let _ = registry.finish_with_result(&operation_id, state, result);
                }
            });
            return ApiResponse::json(202, json!({ "operation": operation_response(operation) }));
        }
        self.import_persist_response(refine_dir, body)
    }

    pub(super) fn import_persist_background_response(
        &self,
        refine_dir: PathBuf,
        body: Value,
        registry: &FileOperationRegistry,
        operation_id: &str,
    ) -> ApiResponse {
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let draft_total = drafts.len();
        let destination = import_feature_destination(&body);
        let mut observer = WebImportPersistObserver {
            registry,
            operation_id,
        };
        let service = FileImportService::new(&refine_dir);
        match service.persist_with_destination(drafts, destination, &mut observer) {
            Ok(result) => self.import_persist_success_response(&refine_dir, result),
            Err(failure) if failure.kind == ImportPersistFailureKind::Cancelled => {
                if let Err(error) = self.refresh_projection_cache_after_mutation() {
                    let _ = registry.fail_with_error(
                        operation_id,
                        json!({
                            "code": "projection_refresh_failed",
                            "message": error.to_string()
                        }),
                    );
                    return error_response(error);
                }
                let _ = registry.update_progress(
                    operation_id,
                    json!({
                        "message": "Import cancelled",
                        "completed": 0,
                        "total": draft_total
                    }),
                );
                ApiResponse::json(499, json!({"cancelled": true}))
            }
            Err(failure) => self.import_persist_failure_response(failure),
        }
    }

    pub(crate) fn promote_backlog_after_mutation(&self) -> Result<usize, RefineError> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(0);
        };
        let Some(target_root) = self.target_root() else {
            return Ok(0);
        };
        WorkflowEngine::with_target_root(runtime_root, target_root).promote_backlog_to_todo()
    }

    pub(super) fn import_persist_response(&self, refine_dir: PathBuf, body: Value) -> ApiResponse {
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let destination = import_feature_destination(&body);
        let service = FileImportService::new(&refine_dir);
        match service.persist_with_destination(drafts, destination, &mut ()) {
            Ok(result) => self.import_persist_success_response(&refine_dir, result),
            Err(failure) => self.import_persist_failure_response(failure),
        }
    }

    fn import_persist_success_response(
        &self,
        refine_dir: &std::path::Path,
        result: crate::tools::product::imports::ImportPersistResult,
    ) -> ApiResponse {
        let service = self.work_item_service(refine_dir);
        let mut failures = Vec::new();
        let promoted = match self.promote_backlog_after_mutation() {
            Ok(count) => count,
            Err(error) => {
                failures.push(json!({
                    "index": 0,
                    "name": "workflow",
                    "message": error.to_string()
                }));
                0
            }
        };
        let created = result
            .goal_ids
            .iter()
            .filter_map(|goal_id| service.show_goal_summary(goal_id).ok())
            .collect::<Vec<_>>();
        let feature = result
            .feature_id
            .as_deref()
            .and_then(|feature_id| service.show_feature_summary(feature_id).ok())
            .as_ref()
            .map(feature_import_response)
            .unwrap_or(Value::Null);

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "goals": created.iter().map(|goal| &goal.goal).collect::<Vec<_>>(),
                "promoted": promoted,
                "failures": failures,
                "duplicate_actions": result.duplicate_actions,
                "feature": feature
            }),
        )
    }

    fn import_persist_failure_response(
        &self,
        failure: crate::tools::product::imports::ImportPersistFailure,
    ) -> ApiResponse {
        let message = failure
            .error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "import persistence was cancelled".to_string());
        ApiResponse::json(
            207,
            json!({
                "ok": false,
                "count": 0,
                "created": [],
                "goals": [],
                "promoted": 0,
                "failures": [{
                    "index": failure.failed_index.unwrap_or(0),
                    "name": failure.failed_name.unwrap_or_else(|| "import".to_string()),
                    "message": message
                }],
                "duplicate_actions": failure.duplicate_actions,
                "feature": Value::Null
            }),
        )
    }
}
