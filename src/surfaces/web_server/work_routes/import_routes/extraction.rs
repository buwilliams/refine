use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_import_extract(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "extract imported Goals");
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("extract imported Goals in the background");
            };
            let purpose = body
                .get("purpose")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("import");
            let owner = if purpose == "plan" {
                "import:extract:plan"
            } else {
                "import:extract"
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register(owner) {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": if purpose == "plan" {
                        "Extracting Plan Feature and Goals"
                    } else {
                        "Extracting import drafts"
                    },
                    "completed": 0,
                    "total": 1
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let server = self.clone();
            let operation_id = operation.id.clone();
            let runtime_root = runtime_root.clone();
            let plan_purpose = purpose == "plan";
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = server.import_extract_response(refine_dir, body);
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
                if response.status >= 400 {
                    let error = result
                        .get("error")
                        .cloned()
                        .unwrap_or_else(|| result.clone());
                    let _ = registry.fail_with_error(&operation_id, error);
                    return;
                }
                let draft_count = result
                    .get("drafts")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let _ = registry.update_progress(
                    &operation_id,
                    json!({
                        "message": if plan_purpose {
                            "Plan Feature and Goal drafts extracted"
                        } else {
                            "Import drafts extracted"
                        },
                        "completed": 1,
                        "total": 1,
                        "draft_count": draft_count
                    }),
                );
                let _ =
                    registry.finish_with_result(&operation_id, OperationState::Succeeded, result);
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
        self.import_extract_response(refine_dir, body)
    }

    pub(super) fn import_extract_response(&self, refine_dir: PathBuf, body: Value) -> ApiResponse {
        let text = match import_extraction_text(&refine_dir, self.runtime_root.as_deref(), &body) {
            Ok(text) => text,
            Err(error) => return error_response(error),
        };
        let purpose = body
            .get("purpose")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("import");
        let reporter = body.get("reporter").and_then(|value| value.as_str());
        let provider =
            import_provider_from_settings(&refine_dir, self.runtime_root.as_deref(), &body);
        let force_provider = body
            .get("force_provider")
            .or_else(|| body.get("forceProvider"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if purpose == "plan"
            && !force_provider
            && let Some(result) = parse_structured_import_result(&text, reporter)
        {
            return match validate_import_extraction_result(result, purpose) {
                Ok(result) => import_extraction_response(result, &provider, purpose, "input"),
                Err(error) => error_response(error),
            };
        }
        let cwd = self.target_root().map(|path| path.display().to_string());
        let output = match HostAgentProviderService::new().invoke(ProviderInvocation {
            provider: provider.clone(),
            prompt: import_extraction_prompt(&text, purpose),
            session_id: None,
            cwd,
            process_metadata: Default::default(),
        }) {
            Ok(output) => output,
            Err(error) => return error_response(error),
        };
        match parse_provider_import_result(&output, reporter) {
            Ok(result) => match validate_import_extraction_result(result, purpose) {
                Ok(result) => import_extraction_response(result, &provider, purpose, "provider"),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_import_csv_parse(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("parse CSV import in the background");
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("import:csv:parse") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let row_total = body_text(&body)
                .lines()
                .skip(1)
                .filter(|line| !line.trim().is_empty())
                .count();
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": "Preparing CSV import",
                    "completed": 0,
                    "total": row_total
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let runtime_root = runtime_root.clone();
            let body_for_worker = body.clone();
            let operation_id = operation.id.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = match FileImportService::new(PathBuf::new()).parse_csv(
                    body_text(&body_for_worker),
                    body_for_worker
                        .get("reporter")
                        .and_then(|value| value.as_str()),
                ) {
                    Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
                    Err(error) => error_response(error),
                };
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
                if response.status >= 400 {
                    let error = result
                        .get("error")
                        .cloned()
                        .unwrap_or_else(|| result.clone());
                    let _ = registry.fail_with_error(&operation_id, error);
                    return;
                }
                let draft_count = result
                    .get("drafts")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let _ = registry.update_progress(
                    &operation_id,
                    json!({
                        "message": "CSV import prepared",
                        "completed": draft_count,
                        "total": row_total,
                        "draft_count": draft_count
                    }),
                );
                let _ =
                    registry.finish_with_result(&operation_id, OperationState::Succeeded, result);
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
        match FileImportService::new(PathBuf::new()).parse_csv(
            body_text(&body),
            body.get("reporter").and_then(|value| value.as_str()),
        ) {
            Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_import_dedup(&self, request: ApiRequest) -> ApiResponse {
        let Some(body) = request.body.as_ref() else {
            return error_response(RefineError::InvalidInput(
                "body.drafts must be an array".to_string(),
            ));
        };
        let drafts = match import_drafts_from_value(body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let mut matches = Vec::new();
        for (index, draft) in drafts.iter().enumerate() {
            let mut needles = vec![normalized_dedup_text(&[
                draft.name.as_str(),
                draft.prompt.as_str(),
            ])];
            needles.retain(|needle| !needle.is_empty());
            if needles.is_empty() {
                continue;
            }
            if let Some(existing) = projection.goals.values().find(|goal| {
                let haystack = normalized_dedup_text(&[
                    goal.goal.name.as_str(),
                    goal.searchable_text.as_str(),
                    goal.goal.id.as_str(),
                ]);
                needles.iter().any(|needle| {
                    haystack == *needle || (!haystack.is_empty() && haystack.contains(needle))
                })
            }) {
                matches.push(json!({
                    "index": index + 1,
                    "score": 1.0,
                    "match": {
                        "id": existing.goal.id,
                        "name": existing.goal.name,
                        "status": existing.goal.status,
                        "priority": existing.goal.priority,
                        "reporter": existing.goal.reporter
                    }
                }));
            }
        }
        ApiResponse::json(
            200,
            json!({
                "matches": matches,
                "threshold": 1.0,
                "algorithm": "normalized_exact"
            }),
        )
    }
}
