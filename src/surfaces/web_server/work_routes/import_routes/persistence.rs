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
        let service = self.work_item_service(&refine_dir);
        let mut failures = Vec::new();
        let mut feature_response = serde_json::Value::Null;
        let feature_id = match import_destination_feature_id(&service, &body) {
            Ok(feature) => {
                feature_response = feature
                    .as_ref()
                    .map(feature_import_response)
                    .unwrap_or(serde_json::Value::Null);
                feature.map(|feature| feature.feature.id)
            }
            Err(error) => {
                failures.push(json!({
                    "index": 0,
                    "name": "feature",
                    "message": error.to_string()
                }));
                None
            }
        };
        let mut created_goal_ids = Vec::new();
        let mut duplicate_actions = ImportDuplicateActions::default();
        if failures.is_empty() {
            let mut context = ImportPersistContext {
                feature_id: feature_id.as_deref(),
                registry,
                operation_id,
                created_goal_ids: &mut created_goal_ids,
                duplicate_actions: &mut duplicate_actions,
            };
            match self.persist_import_drafts_incrementally(&service, drafts, &mut context) {
                Ok(()) => {}
                Err(ImportPersistWorkerError::Cancelled) => {
                    rollback_import_goals(&service, &created_goal_ids);
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
                    return ApiResponse::json(499, json!({"cancelled": true}));
                }
                Err(ImportPersistWorkerError::Failed(error)) => {
                    failures.push(json!({
                        "index": 0,
                        "name": "import",
                        "message": error.to_string()
                    }));
                }
            }
        }
        let mut promoted = 0;
        if failures.is_empty() {
            match self.promote_backlog_after_mutation() {
                Ok(count) => promoted = count,
                Err(error) => failures.push(json!({
                    "index": 0,
                    "name": "workflow",
                    "message": error.to_string()
                })),
            }
        }
        let created = created_goal_ids
            .iter()
            .filter_map(|goal_id| service.show_goal_summary(goal_id).ok())
            .collect::<Vec<_>>();
        if let Some(feature_id) = feature_id.as_deref()
            && let Ok(feature) = service.show_feature_summary(feature_id)
        {
            feature_response = feature_import_response(&feature);
        }

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "goals": created.iter().map(|goal| &goal.goal).collect::<Vec<_>>(),
                "promoted": promoted,
                "failures": failures,
                "duplicate_actions": duplicate_actions.to_json(),
                "feature": feature_response
            }),
        )
    }

    pub(super) fn persist_import_drafts_incrementally(
        &self,
        service: &FileWorkItemService,
        drafts: Vec<ImportDraft>,
        context: &mut ImportPersistContext<'_>,
    ) -> Result<(), ImportPersistWorkerError> {
        if let Some(feature_id) = context.feature_id {
            service
                .show_feature_summary(feature_id)
                .map_err(ImportPersistWorkerError::Failed)?;
        }
        let total = drafts.len();
        let mut created_drafts = Vec::new();
        for draft in drafts {
            if import_operation_cancelled(context.registry, context.operation_id) {
                return Err(ImportPersistWorkerError::Cancelled);
            }
            if let Some(goal_id) = persist_import_draft_with_duplicate_decision(
                service,
                &draft,
                context.feature_id,
                context.duplicate_actions,
                context.created_goal_ids,
                &mut created_drafts,
            )
            .map_err(ImportPersistWorkerError::Failed)?
            {
                let _ = goal_id;
            }
            let _ = context.registry.update_progress(
                context.operation_id,
                json!({
                    "message": "Saving import",
                    "completed": context.created_goal_ids.len(),
                    "total": total
                }),
            );
            thread::sleep(Duration::from_millis(5));
        }
        if import_operation_cancelled(context.registry, context.operation_id) {
            return Err(ImportPersistWorkerError::Cancelled);
        }
        if let Some(feature_id) = context.feature_id {
            order_feature_dependency_drafts(service, feature_id, &created_drafts)
                .map_err(ImportPersistWorkerError::Failed)?;
        }
        Ok(())
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
        let service = self.work_item_service(&refine_dir);
        let mut failures = Vec::new();
        let mut feature_response = serde_json::Value::Null;
        let feature_id = match import_destination_feature_id(&service, &body) {
            Ok(feature) => {
                feature_response = feature
                    .as_ref()
                    .map(feature_import_response)
                    .unwrap_or(serde_json::Value::Null);
                feature.map(|feature| feature.feature.id)
            }
            Err(error) => {
                failures.push(json!({
                    "index": 0,
                    "name": "feature",
                    "message": error.to_string()
                }));
                None
            }
        };
        let import_result = if failures.is_empty() {
            let mut goal_ids = Vec::new();
            let mut created_drafts = Vec::new();
            let mut duplicate_actions = ImportDuplicateActions::default();
            let result: Result<crate::tools::product::imports::ImportPersistResult, RefineError> =
                (|| {
                    if let Some(feature_id) = feature_id.as_deref() {
                        service.show_feature_summary(feature_id)?;
                    }
                    for draft in drafts {
                        if let Some(goal_id) = persist_import_draft_with_duplicate_decision(
                            &service,
                            &draft,
                            feature_id.as_deref(),
                            &mut duplicate_actions,
                            &mut goal_ids,
                            &mut created_drafts,
                        )? {
                            let _ = goal_id;
                        }
                    }
                    if let Some(feature_id) = feature_id.as_deref() {
                        order_feature_dependency_drafts(&service, feature_id, &created_drafts)?;
                    }
                    Ok(crate::tools::product::imports::ImportPersistResult {
                        created: goal_ids.len(),
                        goal_ids,
                        feature_id: feature_id.clone(),
                    })
                })();
            match result {
                Ok(result) => Some((result, duplicate_actions)),
                Err(error) => {
                    failures.push(json!({
                        "index": 0,
                        "name": "import",
                        "message": error.to_string()
                    }));
                    None
                }
            }
        } else {
            None
        };
        let mut promoted = 0;
        if failures.is_empty() {
            match self.promote_backlog_after_mutation() {
                Ok(count) => promoted = count,
                Err(error) => failures.push(json!({
                    "index": 0,
                    "name": "workflow",
                    "message": error.to_string()
                })),
            }
        }
        let created = import_result
            .as_ref()
            .map(|(result, _)| {
                result
                    .goal_ids
                    .iter()
                    .filter_map(|goal_id| service.show_goal_summary(goal_id).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(feature_id) = feature_id.as_deref()
            && let Ok(feature) = service.show_feature_summary(feature_id)
        {
            feature_response = feature_import_response(&feature);
        }

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "goals": created.iter().map(|goal| &goal.goal).collect::<Vec<_>>(),
                "promoted": promoted,
                "failures": failures,
                "duplicate_actions": import_result
                    .as_ref()
                    .map(|(_, actions)| actions.to_json())
                    .unwrap_or_else(|| ImportDuplicateActions::default().to_json()),
                "feature": feature_response
            }),
        )
    }
}
