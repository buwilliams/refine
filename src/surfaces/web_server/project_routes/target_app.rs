use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_target_app_status(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.snapshot())
        {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_target_app_health(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.health())
        {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_target_app_action(&self, request: ApiRequest) -> ApiResponse {
        let kind = request
            .path
            .strip_prefix("/target-app/")
            .unwrap_or("status")
            .to_string();
        if !matches!(kind.as_str(), "start" | "stop" | "build") {
            return error_response(RefineError::InvalidInput(format!(
                "unknown target-app action {kind}"
            )));
        }
        self.queue_target_app_action(kind)
    }

    pub(crate) fn queue_target_app_action(&self, kind: String) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run target-app lifecycle action");
        };
        let registry = FileOperationRegistry::new(runtime_root);
        let operation = match registry
            .register_exclusive_with_request("target-app:lifecycle", json!({"kind": kind}))
        {
            Ok(operation) => operation,
            Err(error) => return error_response(error),
        };
        let _ = registry.update_progress(
            &operation.id,
            json!({"message": format!("Target application {kind} is running")}),
        );
        let operation = registry.status(&operation.id).unwrap_or(operation);
        let operation_id = operation.id.clone();
        let runtime_root = runtime_root.clone();
        let server = self.clone();
        thread::spawn(move || {
            let registry = FileOperationRegistry::new(&runtime_root);
            let result = server
                .target_app_service()
                .and_then(|service| match kind.as_str() {
                    "start" => service.start(),
                    "stop" => service.stop(),
                    "build" => service.build(),
                    _ => unreachable!("target-app action was validated before launch"),
                });
            match result {
                Ok(snapshot) => {
                    let queued = kind == "build"
                        && snapshot.ok
                        && snapshot
                            .last_operation
                            .as_ref()
                            .is_some_and(|operation| operation.kind == "build");
                    let mut result = server.target_app_response(snapshot);
                    if kind == "build" {
                        result["queued"] = json!(queued);
                    }
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
                            "code": "target_app_action_failed",
                            "message": error.to_string(),
                            "kind": kind
                        }),
                    );
                }
            }
        });
        ApiResponse::json(202, json!({"operation": operation_response(operation)}))
    }

    pub(crate) fn handle_target_app_generate_instructions(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("generate target-app config in the background");
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("target-app:generate") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": "Generating target-app config with AI"
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let server = self.clone();
            let runtime_root = runtime_root.clone();
            let operation_id = operation.id.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = server.target_app_generate_response(&body, true);
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
                    let error = result.get("error").cloned().unwrap_or_else(|| {
                        json!({
                            "message": "Target-app config generation failed",
                            "details": result
                        })
                    });
                    let _ = registry.fail_with_error(&operation_id, error);
                } else {
                    let _ = registry.update_progress(
                        &operation_id,
                        json!({
                            "message": "Generated target-app config"
                        }),
                    );
                    let _ = registry.finish_with_result(
                        &operation_id,
                        OperationState::Succeeded,
                        result,
                    );
                }
                let _ = server.refresh_projection_cache_after_mutation();
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
        self.target_app_generate_response(&body, false)
    }

    pub(crate) fn target_app_generate_response(
        &self,
        body: &Value,
        persist_settings: bool,
    ) -> ApiResponse {
        let service = match self.target_app_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        let mut provider = String::new();
        let mut source = "local".to_string();
        let mut raw = String::new();
        let config = match self.current_refine_dir() {
            Ok(Some(refine_dir)) => {
                provider = configured_provider_from_settings(
                    &refine_dir,
                    self.runtime_root.as_deref(),
                    body,
                );
                match self.agent_provider_service().invoke(ProviderInvocation {
                    provider: provider.clone(),
                    prompt: target_app_generation_prompt(&service.target_root),
                    session_id: None,
                    cwd: Some(service.target_root.display().to_string()),
                    process_metadata: Default::default(),
                }) {
                    Ok(output) => {
                        raw = output.clone();
                        if let Some(config) = parse_generated_target_app_config(&output) {
                            source = "provider".to_string();
                            Ok(config)
                        } else {
                            service.generate_config()
                        }
                    }
                    Err(_) => service.generate_config(),
                }
            }
            Ok(None) => service.generate_config(),
            Err(error) => Err(error),
        };
        match config {
            Ok(config) => {
                let settings = target_app_generated_settings(&config);
                if persist_settings {
                    match self.current_refine_dir() {
                        Ok(Some(refine_dir)) => {
                            if let Err(error) = self.settings_service(&refine_dir).update(&settings)
                            {
                                return error_response(error);
                            }
                            if let Err(error) = self.apply_current_runtime_settings() {
                                return error_response(error);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => return error_response(error),
                    }
                }
                ApiResponse::json(
                    200,
                    json!({
                        "ok": true,
                        "config": config,
                        "settings": settings,
                        "provider": provider,
                        "source": source,
                        "raw": raw,
                        "message": if source == "provider" {
                            "Generated target-app configuration with the configured provider."
                        } else {
                            "Generated target-app configuration from local project files."
                        }
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_target_app_build_queue(&self) -> ApiResponse {
        self.queue_target_app_action("build".to_string())
    }
}
