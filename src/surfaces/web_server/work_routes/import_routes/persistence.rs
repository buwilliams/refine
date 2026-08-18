use super::*;

const IMPORT_ROLLBACK_INCOMPLETE_CODE: &str = "import_rollback_incomplete";
const IMPORT_PARTIAL_FAILURE_KIND: &str = "partial_failure";
const IMPORT_RECOVERY_GUIDANCE: &str = "Inspect the unrecovered Goal and Feature IDs, resolve the recorded rollback failures, then delete the remaining import-created records or retry cleanup.";

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
            let operation = match registry.register_with_request(
                "import:persist",
                json!({"defer_cancellation_terminal": true}),
            ) {
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
                let partial_failure = result.get("code").and_then(Value::as_str)
                    == Some(IMPORT_ROLLBACK_INCOMPLETE_CODE);
                let _ = registry.update_progress(
                    &operation_id,
                    json!({
                        "message": if partial_failure {
                            "Import cleanup incomplete"
                        } else {
                            "Import saved"
                        },
                        "completed": count,
                        "total": draft_total
                    }),
                );
                let refresh_result = server.refresh_projection_cache_after_mutation();
                if partial_failure {
                    if let Err(error) = refresh_result
                        && let Some(object) = result.as_object_mut()
                    {
                        object.insert(
                            "projection_refresh_error".to_string(),
                            json!(error.to_string()),
                        );
                    }
                    let error = import_partial_failure_operation_error(&result);
                    let _ = registry.fail_with_partial_result(&operation_id, error, result);
                    return;
                }
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
            Err(failure)
                if failure.kind == ImportPersistFailureKind::Cancelled
                    && failure.rollback.rollback_failures.is_empty() =>
            {
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
                if let Err(error) = registry.settle_cancellation(operation_id) {
                    let _ = registry.fail_with_error(
                        operation_id,
                        json!({
                            "code": "import_cancellation_settlement_failed",
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
        result: crate::application::imports::ImportPersistResult,
    ) -> ApiResponse {
        let service = self.work_item_service(refine_dir);
        let mut failures = Vec::new();
        let promoted = match self.promote_backlog_after_mutation() {
            Ok(count) => count,
            Err(error) => {
                failures.push(json!({
                    "index": 1,
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
        failure: crate::application::imports::ImportPersistFailure,
    ) -> ApiResponse {
        if !failure.rollback.rollback_failures.is_empty() {
            return import_partial_failure_response(failure);
        }
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
                    "index": import_api_draft_index(
                        failure.failed_draft_index_zero_based
                    ),
                    "name": failure.failed_name.unwrap_or_else(|| "import".to_string()),
                    "message": message
                }],
                "duplicate_actions": failure.duplicate_actions,
                "feature": Value::Null
            }),
        )
    }
}

fn import_api_draft_index(failed_draft_index_zero_based: Option<usize>) -> usize {
    failed_draft_index_zero_based
        .and_then(|index| index.checked_add(1))
        .unwrap_or(1)
}

fn import_partial_failure_response(
    failure: crate::application::imports::ImportPersistFailure,
) -> ApiResponse {
    let message = failure
        .error
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "import persistence was cancelled".to_string());
    let created_goal_ids = failure.rollback.created_goal_ids;
    let rolled_back_goal_ids = failure.rollback.rolled_back_goal_ids;
    let unrecovered_goal_ids = created_goal_ids
        .iter()
        .filter(|goal_id| !rolled_back_goal_ids.contains(goal_id))
        .cloned()
        .collect::<Vec<_>>();
    let created_feature_ids = failure
        .rollback
        .created_feature_id
        .into_iter()
        .collect::<Vec<_>>();
    let rolled_back_feature_ids = failure
        .rollback
        .rolled_back_feature_id
        .into_iter()
        .collect::<Vec<_>>();
    let unrecovered_feature_ids = created_feature_ids
        .iter()
        .filter(|feature_id| !rolled_back_feature_ids.contains(feature_id))
        .cloned()
        .collect::<Vec<_>>();
    let failure_index = import_api_draft_index(failure.failed_draft_index_zero_based);
    let failure_name = failure.failed_name.unwrap_or_else(|| "import".to_string());
    let rollback_failures = failure.rollback.rollback_failures;
    ApiResponse::json(
        500,
        json!({
            "ok": false,
            "cancelled": false,
            "code": IMPORT_ROLLBACK_INCOMPLETE_CODE,
            "kind": IMPORT_PARTIAL_FAILURE_KIND,
            "message": format!(
                "{message}; import rollback was incomplete and manual recovery is required"
            ),
            "count": 0,
            "created": [],
            "goals": [],
            "promoted": 0,
            "failures": [{
                "index": failure_index,
                "name": failure_name,
                "code": IMPORT_ROLLBACK_INCOMPLETE_CODE,
                "kind": IMPORT_PARTIAL_FAILURE_KIND,
                "message": message
            }],
            "duplicate_actions": failure.duplicate_actions,
            "feature": Value::Null,
            "created_goal_ids": created_goal_ids,
            "created_feature_ids": created_feature_ids,
            "rolled_back_goal_ids": rolled_back_goal_ids,
            "rolled_back_feature_ids": rolled_back_feature_ids,
            "unrecovered_goal_ids": unrecovered_goal_ids,
            "unrecovered_feature_ids": unrecovered_feature_ids,
            "rollback_failures": rollback_failures,
            "recovery_guidance": IMPORT_RECOVERY_GUIDANCE
        }),
    )
}

fn import_partial_failure_operation_error(result: &Value) -> Value {
    json!({
        "code": IMPORT_ROLLBACK_INCOMPLETE_CODE,
        "kind": IMPORT_PARTIAL_FAILURE_KIND,
        "message": result.get("message").cloned().unwrap_or_else(|| {
            json!("Import rollback was incomplete and manual recovery is required")
        }),
        "details": result
    })
}
