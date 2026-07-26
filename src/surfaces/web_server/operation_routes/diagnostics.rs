use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_diagnostics(&self) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read diagnostics");
        }
        match self.diagnostics_value(false) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_support_bundle(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "export support bundle");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("export support bundle");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let redact_secrets = body
            .get("redact_secrets")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let repo_root = self
            .target_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        match FileSupportBundleService::new(refine_dir, runtime_root.clone(), repo_root)
            .export(redact_secrets)
        {
            Ok(bundle) => ApiResponse::json(200, json!(bundle)),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn warm_diagnostics_cache(&self) -> RefineResult<()> {
        if self.runtime_root.is_none() {
            return Ok(());
        }
        self.diagnostics_value(true).map(|_| ())
    }

    fn diagnostics_value(&self, refresh: bool) -> RefineResult<Value> {
        let Some(runtime_root) = &self.runtime_root else {
            return Err(RefineError::InvalidInput(
                "runtime root is required to read diagnostics".to_string(),
            ));
        };
        let repo_root = self
            .target_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let target_root = self.current_target_root()?;
        let refine_dir = self.current_refine_dir()?;
        let cache_key = diagnostics_cache_key(runtime_root, refine_dir.as_ref(), &repo_root);
        if !refresh {
            let cached = {
                let cache = DIAGNOSTICS_CACHE
                    .get_or_init(|| Mutex::new(BTreeMap::new()))
                    .lock()
                    .map_err(|_| {
                        RefineError::Io("diagnostics cache lock was poisoned".to_string())
                    })?;
                cache.get(&cache_key).map(|entry| entry.value.clone())
            };
            if let Some(mut value) = cached {
                let process_summary = live_process_summary(runtime_root, refine_dir.as_deref())?;
                value["processes"] = process_status_value(&process_summary);
                return Ok(value);
            }
        }
        let projection = self.current_projection_with_runtime()?;
        let doctor =
            FileDiagnosticsService::new(target_root, runtime_root.clone(), repo_root).doctor()?;
        let provider = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        let process =
            process_status_value(&live_process_summary(runtime_root, refine_dir.as_deref())?);
        let value = json!({
            "reachable": true,
            "backend": {
                "process_model": "supervisor",
                "native": true
            },
            "provider": provider,
            "processes": process,
            "doctor": doctor
        });
        DIAGNOSTICS_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| RefineError::Io("diagnostics cache lock was poisoned".to_string()))?
            .insert(
                cache_key,
                DiagnosticsCacheEntry {
                    value: value.clone(),
                },
            );
        Ok(value)
    }
}
