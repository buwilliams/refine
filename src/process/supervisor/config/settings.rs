use super::*;

#[derive(Clone, Debug)]
pub struct FileSettingsService {
    pub refine_dir: PathBuf,
    pub active_root: Option<PathBuf>,
    pub active_node_id_override: Option<String>,
}

impl FileSettingsService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            active_root: None,
            active_node_id_override: None,
        }
    }

    pub fn with_active_root(
        refine_dir: impl Into<PathBuf>,
        active_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            active_root: Some(active_root.into()),
            active_node_id_override: None,
        }
    }

    pub fn for_node(refine_dir: impl Into<PathBuf>, node_id: impl Into<String>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            active_root: None,
            active_node_id_override: Some(node_id.into()),
        }
    }

    pub fn path(&self) -> PathBuf {
        FileNodeRegistryService::new(&self.refine_dir).registry_path()
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        Ok(serde_json::json!({"settings": self.load()?}))
    }

    pub fn update(&self, body: &serde_json::Value) -> RefineResult<serde_json::Value> {
        let updates = normalize_settings_patch(body)?;
        let mut current = self.load()?;
        let mut updated_test_command = false;
        let mut updated_test_commands = false;
        for (key, value) in updates {
            current.insert(key.clone(), value);
            if key == "target_app_test_command" {
                updated_test_command = true;
            } else if key == "target_app_test_commands" {
                updated_test_commands = true;
            }
        }
        if updated_test_command || updated_test_commands {
            sync_target_app_test_settings(&mut current, updated_test_commands)?;
        }
        self.validate(&current)?;
        self.write(&current)?;
        Ok(serde_json::json!({"ok": true, "settings": current}))
    }

    pub fn validate_update(body: &serde_json::Value) -> RefineResult<()> {
        normalize_settings_patch(body).map(|_| ())
    }

    fn write(&self, settings: &JsonObject) -> RefineResult<()> {
        let service = self.node_registry_service();
        let active_node_id = self.active_node_id()?;
        let mut registry = service.load_registry()?;
        let now = now_timestamp();
        if !registry.nodes.iter().any(|node| node.id == active_node_id) {
            registry.nodes.push(settings_node(&active_node_id, &now));
        }
        let Some(node) = registry
            .nodes
            .iter_mut()
            .find(|node| node.id == active_node_id)
        else {
            return Err(RefineError::NotFound(format!(
                "node {active_node_id} was not found"
            )));
        };
        node.settings = settings.clone();
        node.settings.remove("quality_timing");
        node.updated_at = now;
        service.save_registry(&registry)
    }

    fn node_registry_service(&self) -> FileNodeRegistryService {
        match &self.active_root {
            Some(active_root) => {
                FileNodeRegistryService::with_active_root(&self.refine_dir, active_root)
            }
            None => FileNodeRegistryService::new(&self.refine_dir),
        }
    }

    fn active_node_id(&self) -> RefineResult<String> {
        if let Some(node_id) = &self.active_node_id_override {
            return Ok(node_id.clone());
        }
        self.node_registry_service().active_node_id()
    }

    fn legacy_path(&self) -> PathBuf {
        self.refine_dir.join(SETTINGS_FILE)
    }

    fn remove_legacy_settings(&self) -> RefineResult<()> {
        let path = self.legacy_path();
        if !path.exists() {
            return Ok(());
        }
        fs::remove_file(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to remove legacy settings {}: {error}",
                path.display()
            ))
        })
    }
}

impl ConfigService for FileSettingsService {
    fn load(&self) -> RefineResult<JsonObject> {
        let service = self.node_registry_service();
        let active_node_id = self.active_node_id()?;
        let registry = service.load_registry()?;
        let stored = registry
            .nodes
            .iter()
            .find(|node| node.id == active_node_id)
            .map(|node| node.settings.clone())
            .unwrap_or_default();
        let mut settings = default_settings();
        let mut migrated = false;
        self.remove_legacy_settings()?;
        for (key, value) in stored {
            if key == "paused" {
                // Pause is runtime process control, not durable project configuration.
                migrated = true;
            } else if allowed_settings().contains(key.as_str()) {
                settings.insert(key.clone(), Value::String(normalize_setting(&key, &value)?));
            } else if let Some(new_key) = legacy_setting_key(&key) {
                settings.insert(
                    new_key.to_string(),
                    Value::String(normalize_setting(new_key, &value)?),
                );
                migrated = true;
            } else if key == "quality_timing"
                || key == RETIRED_SUPERVISOR_STALL_KEY
                || is_retired_development_request_setting(&key)
            {
                migrated = true;
            }
        }
        if sync_target_app_test_settings(&mut settings, false)? {
            migrated = true;
        }
        if migrated {
            self.write(&settings)?;
        }
        Ok(settings)
    }

    fn validate(&self, config: &JsonObject) -> RefineResult<()> {
        let allowed = allowed_settings();
        for key in config.keys() {
            if !allowed.contains(key.as_str()) {
                return Err(RefineError::InvalidInput(format!("unknown setting: {key}")));
            }
        }
        Ok(())
    }

    fn merge(&self, mut base: JsonObject, overlay: JsonObject) -> RefineResult<JsonObject> {
        for (key, value) in overlay {
            base.insert(key, value);
        }
        self.validate(&base)?;
        Ok(base)
    }
}
