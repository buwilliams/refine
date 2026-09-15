use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_agents(&self) -> ApiResponse {
        match self.provider_status_value() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_agent_diagnostics(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "diagnostics") else {
            return provider_id_required();
        };
        match (match self.agent_provider_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        })
        .diagnose(&provider)
        {
            Ok(diagnostics) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "diagnostics": diagnostics
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_agent_configure(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "configure") else {
            return provider_id_required();
        };
        match (match self.agent_provider_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        })
        .configure(&provider)
        {
            Ok(()) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "configured": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_agent_invoke(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "invoke") else {
            return provider_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(prompt) = body.get("prompt").and_then(Value::as_str) else {
            return error_response(RefineError::InvalidInput(
                "agent invoke requires prompt".to_string(),
            ));
        };
        let cwd = body
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let root = match self.current_refine_dir() {
            Ok(root) => root,
            Err(error) => return error_response(error),
        };
        let _templates =
            match crate::application::templates::TemplateScope::for_root(root.as_deref()) {
                Ok(scope) => scope,
                Err(error) => return error_response(error),
            };
        let prompt = match crate::application::agent_io::prompts::render(
            crate::application::agent_io::prompts::PromptTemplate::DirectAgent,
            &[("message", prompt)],
        ) {
            Ok(prompt) => prompt,
            Err(error) => return error_response(error),
        };
        match (match self.agent_provider_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        })
        .invoke(ProviderInvocation {
            stall_timeout_seconds: None,
            provider: provider.to_string(),
            prompt: prompt.to_string(),
            session_id: None,
            cwd,
            process_metadata: Default::default(),
        }) {
            Ok(output) => ApiResponse::json(200, json!({"ok": true, "output": output})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_agent_resume(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "resume") else {
            return provider_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(session_id) = body.get("session_id").and_then(Value::as_str) else {
            return error_response(RefineError::InvalidInput(
                "agent resume requires session_id".to_string(),
            ));
        };
        match (match self.agent_provider_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        })
        .resume(&provider, session_id)
        {
            Ok(output) => ApiResponse::json(200, json!({"ok": true, "output": output})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_agent_authenticate(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let suffix = if request.path.ends_with("/authenticate") {
            "authenticate"
        } else {
            "auth"
        };
        let Some(provider) = agent_provider_from_path(&request.path, suffix) else {
            return provider_id_required();
        };
        match (match self.agent_provider_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        })
        .authenticate(&provider)
        {
            Ok(()) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "authenticated": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_recheck_auth(&self) -> ApiResponse {
        self.handle_agents()
    }

    pub(in crate::surfaces::web_server) fn handle_agent_secrets_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("inspect secret storage");
        };
        let store = NativeSecretStore::new(runtime_root);
        ApiResponse::json(200, json!({"secret_store": store.backend_status()}))
    }

    pub(in crate::surfaces::web_server) fn handle_agent_secrets_list(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("list secrets");
        };
        let store = NativeSecretStore::new(runtime_root);
        match store.list_secrets() {
            Ok(secrets) => ApiResponse::json(200, json!({"secrets": secrets})),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_agent_secret(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("manage secrets");
        };
        let Some((scope, name)) = secret_scope_name_from_path(&request.path) else {
            return error_response(RefineError::InvalidInput(
                "secret path must be /agents/secrets/{scope}/{name}".to_string(),
            ));
        };
        let store = NativeSecretStore::new(runtime_root);
        match request.method.as_str() {
            "GET" => match store.get_secret(&scope, &name) {
                Ok(secret) => ApiResponse::json(
                    200,
                    json!({"secret": secret.metadata, "value": secret.value}),
                ),
                Err(error) => error_response(error),
            },
            "PUT" | "POST" => {
                let body = request.body.unwrap_or_else(|| json!({}));
                let value = body
                    .get("value")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                match store.put_secret(&scope, &name, value) {
                    Ok(secret) => ApiResponse::json(200, json!({"secret": secret})),
                    Err(error) => error_response(error),
                }
            }
            "DELETE" => match store.delete_secret(&scope, &name) {
                Ok(secret) => ApiResponse::json(200, json!({"deleted": secret})),
                Err(error) => error_response(error),
            },
            _ => ApiResponse::json(
                405,
                json!({
                    "error": {
                        "code": "method_not_allowed",
                        "message": "secret route supports GET, PUT, POST, and DELETE"
                    }
                }),
            ),
        }
    }
}
