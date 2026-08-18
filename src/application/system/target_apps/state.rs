use super::*;

impl FileTargetAppService {
    pub(super) fn settings(&self) -> RefineResult<JsonObject> {
        FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()
    }

    pub(super) fn state_path(&self) -> PathBuf {
        self.runtime_root.join(TARGET_APP_STATE_FILE)
    }

    pub(super) fn load_snapshot(&self) -> RefineResult<TargetAppSnapshot> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(TargetAppSnapshot::default());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read target-app state {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse target-app state {}: {error}",
                path.display()
            ))
        })
    }

    pub(super) fn save_snapshot(&self, snapshot: &TargetAppSnapshot) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(snapshot).map_err(|error| {
            RefineError::Serialization(format!("failed to encode target-app state: {error}"))
        })?;
        let state_path = self.state_path();
        let temp_path = self.snapshot_temp_path();
        fs::write(&temp_path, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write target-app state temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        fs::rename(&temp_path, &state_path).map_err(|error| {
            let _ = fs::remove_file(&temp_path);
            RefineError::Io(format!(
                "failed to replace target-app state {}: {error}",
                state_path.display()
            ))
        })
    }

    pub(super) fn snapshot_temp_path(&self) -> PathBuf {
        let nanos = Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| Utc::now().timestamp_micros() * 1000);
        self.runtime_root.join(format!(
            ".{TARGET_APP_STATE_FILE}.{}.{}.tmp",
            std::process::id(),
            nanos
        ))
    }
}
