use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_settings_get(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read settings");
        match self.settings_service(refine_dir).list_response() {
            Ok(value) => ApiResponse::json(200, self.with_runtime_settings(value)),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_settings_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update settings");
        let mut body = request.body.unwrap_or_else(|| json!({}));
        if let Some(paused) = body.get("paused").map(runtime_bool_setting)
            && let Some(runtime_root) = &self.runtime_root
        {
            match self.current_target_root() {
                Ok(Some(target_root)) => {
                    if let Err(error) = WorkflowEngine::with_target_root(runtime_root, target_root)
                        .set_workflow_paused(paused)
                    {
                        return error_response(error);
                    }
                }
                Ok(None) => {
                    if let Err(error) =
                        FileProcessSupervisor::new(runtime_root).set_workflow_paused(paused)
                    {
                        return error_response(error);
                    }
                }
                Err(error) => return error_response(error),
            }
        }
        if let Some(body) = body.as_object_mut() {
            body.remove("paused");
        }
        let settings = self.settings_service(&refine_dir);
        let updated = if body.as_object().is_some_and(|body| body.is_empty()) {
            settings
                .load()
                .map(|settings| json!({"ok": true, "settings": settings}))
        } else {
            settings.update(&body)
        };
        match updated {
            Ok(value) => {
                if let Err(error) = self.apply_current_runtime_settings() {
                    return error_response(error);
                }
                let value = self.with_runtime_settings(value);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn apply_current_runtime_settings(&self) -> RefineResult<()> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(());
        };
        let Some(target_root) = self.current_target_root()? else {
            return Ok(());
        };
        WorkflowEngine::with_target_root(runtime_root, target_root)
            .apply_runtime_settings()
            .map(|_| ())
    }

    pub(crate) fn handle_upgrade_status(&self) -> ApiResponse {
        let current_version = env!("CARGO_PKG_VERSION");
        let latest_version = std::env::var("REFINE_TEST_UPGRADE_LATEST_VERSION")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| current_version.to_string());
        let upgrade_available = latest_version != current_version;
        let local_development = !upgrade_available;
        let command = std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "refine".to_string());
        ApiResponse::json(
            200,
            json!({
                "upgrade": {
                    "available": upgrade_available,
                    "upgrade_available": upgrade_available,
                    "current_version": current_version,
                    "latest_version": latest_version,
                    "launch_mode": current_launch_mode(),
                    "executable_path": current_launch_executable(),
                    "local_development": local_development,
                    "message": if upgrade_available {
                        format!("Refine {latest_version} is available; current version is {current_version}.")
                    } else {
                        format!("Running native Refine {current_version}; remote release discovery is not configured for this build.")
                    },
                    "command": command
                }
            }),
        )
    }

    pub(crate) fn handle_governance_get(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read governance settings");
        match FileGovernanceService::new(refine_dir).load() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_governance_save(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "save governance settings");
        match FileGovernanceService::new(refine_dir)
            .save(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_governance_generate_rules(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "generate governance rules");
        let body = request.body.unwrap_or_else(|| json!({}));
        let product = body
            .get("product")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let constitution = body
            .get("constitution")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if product.is_empty() || constitution.is_empty() {
            return error_response(RefineError::InvalidInput(
                "product and constitution are required".to_string(),
            ));
        }

        let provider =
            configured_provider_from_settings(&refine_dir, self.runtime_root.as_deref(), &body);
        let cwd = self.target_root().map(|path| path.display().to_string());
        let output = match self.agent_provider_service().invoke(ProviderInvocation {
            provider: provider.clone(),
            prompt: governance_generation_prompt(product, constitution),
            session_id: None,
            cwd,
            process_metadata: Default::default(),
        }) {
            Ok(output) => output,
            Err(_) => {
                return match FileGovernanceService::new(&refine_dir).generate_rules(&body) {
                    Ok(mut value) => {
                        if let Some(object) = value.as_object_mut() {
                            object.insert("source".to_string(), json!("static"));
                        }
                        ApiResponse::json(200, value)
                    }
                    Err(error) => error_response(error),
                };
            }
        };
        let mut rules = parse_generated_governance_rules(&output);
        if rules.is_empty() {
            match FileGovernanceService::new(&refine_dir).generate_rules(&body) {
                Ok(value) => {
                    rules = value["rules"].as_array().cloned().unwrap_or_default();
                }
                Err(error) => return error_response(error),
            }
        }
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "provider": provider,
                "source": "provider",
                "rules": rules,
                "raw": output
            }),
        )
    }

    pub(crate) fn handle_guidance_list(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read guidance");
        match FileGuidanceService::new(refine_dir).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_guidance_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update guidance");
        match FileGuidanceService::new(refine_dir)
            .update(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_guidance_add(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "add guidance");
        match FileGuidanceService::new(refine_dir).add(&request.body.unwrap_or_else(|| json!({}))) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_guidance_edit(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "edit guidance");
        let Some(id) = guidance_id_from_path(&request.path) else {
            return guidance_id_required();
        };
        match FileGuidanceService::new(refine_dir)
            .edit(id, &request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_guidance_remove(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "remove guidance");
        let Some(id) = guidance_id_from_path(&request.path) else {
            return guidance_id_required();
        };
        match FileGuidanceService::new(refine_dir)
            .remove(id, &request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_reporters_list(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "list reporters");
        match FileReporterService::new(refine_dir).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_reporter_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create reporters");
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(refine_dir).create(name) {
            Ok(value) => ApiResponse::json(201, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_reporter_rename(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "rename reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(refine_dir).rename(id, name) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_reporter_merge(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "merge reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "/merge") else {
            return reporter_id_required();
        };
        let Some(target_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target_id"))
            .and_then(|value| value.as_u64())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "target_id is required"
                    }
                }),
            );
        };
        match FileReporterService::new(refine_dir).merge(id, target_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_reporter_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        match FileReporterService::new(refine_dir).delete(id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }
}
