use super::*;

impl FileInstallationService {
    pub fn new(runtime_root: impl Into<PathBuf>, current_version: impl Into<String>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            current_version: current_version.into(),
            port: None,
            path_inputs: RuntimePathInputs::from_env(),
        }
    }

    pub fn for_port(
        runtime_root: impl Into<PathBuf>,
        current_version: impl Into<String>,
        port: u16,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            current_version: current_version.into(),
            port: Some(port),
            path_inputs: RuntimePathInputs::from_env(),
        }
    }

    pub fn with_path_inputs(
        runtime_root: impl Into<PathBuf>,
        current_version: impl Into<String>,
        path_inputs: RuntimePathInputs,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            current_version: current_version.into(),
            port: None,
            path_inputs,
        }
    }

    pub fn with_path_inputs_for_port(
        runtime_root: impl Into<PathBuf>,
        current_version: impl Into<String>,
        port: u16,
        path_inputs: RuntimePathInputs,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            current_version: current_version.into(),
            port: Some(port),
            path_inputs,
        }
    }

    pub(super) fn state_root(&self) -> PathBuf {
        match self.port {
            Some(port) => self.runtime_root.join(port.to_string()),
            None => self.runtime_root.clone(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.state_root().join(INSTALL_STATE_FILE)
    }

    pub fn backend_path(&self) -> PathBuf {
        self.state_root().join(INSTALL_BACKEND_FILE)
    }

    pub(super) fn legacy_path(&self) -> Option<PathBuf> {
        self.port
            .map(|_| self.runtime_root.join(INSTALL_STATE_FILE))
    }

    pub(super) fn legacy_backend_path(&self) -> Option<PathBuf> {
        self.port
            .map(|_| self.runtime_root.join(INSTALL_BACKEND_FILE))
    }

    pub(super) fn load(&self) -> RefineResult<InstallStateDocument> {
        let mut path = self.path();
        if !path.exists()
            && let Some(legacy_path) = self.legacy_path()
            && legacy_path.exists()
        {
            path = legacy_path;
        }
        if !path.exists() {
            return Ok(default_state(
                self.default_target(),
                &self.current_version,
                self.port,
            ));
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read install state {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<InstallStateDocument>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse install state {}: {error}",
                path.display()
            ))
        })
    }

    pub(super) fn save(&self, state: &InstallStateDocument) -> RefineResult<()> {
        let state_root = self.state_root();
        fs::create_dir_all(&state_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                state_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode install state: {error}"))
        })?;
        fs::write(self.path(), encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write install state {}: {error}",
                self.path().display()
            ))
        })?;
        if let Some(legacy_path) = self.legacy_path()
            && legacy_path.exists()
        {
            fs::remove_file(&legacy_path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove legacy install state {}: {error}",
                    legacy_path.display()
                ))
            })?;
        }
        Ok(())
    }

    pub(super) fn load_backend(&self) -> RefineResult<Option<InstallBackendRegistration>> {
        let mut path = self.backend_path();
        if !path.exists()
            && let Some(legacy_path) = self.legacy_backend_path()
            && legacy_path.exists()
        {
            path = legacy_path;
        }
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read install backend {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<InstallBackendRegistration>(&bytes)
            .map(Some)
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse install backend {}: {error}",
                    path.display()
                ))
            })
    }

    pub(super) fn save_backend(&self, backend: &InstallBackendRegistration) -> RefineResult<()> {
        let state_root = self.state_root();
        fs::create_dir_all(&state_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                state_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(backend).map_err(|error| {
            RefineError::Serialization(format!("failed to encode install backend: {error}"))
        })?;
        fs::write(self.backend_path(), encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write install backend {}: {error}",
                self.backend_path().display()
            ))
        })?;
        if let Some(legacy_backend_path) = self.legacy_backend_path()
            && legacy_backend_path.exists()
        {
            fs::remove_file(&legacy_backend_path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove legacy install backend {}: {error}",
                    legacy_backend_path.display()
                ))
            })?;
        }
        Ok(())
    }

    pub(super) fn register_backend(
        &self,
        target: InstallTarget,
    ) -> RefineResult<InstallBackendRegistration> {
        let now = now_timestamp();
        let mut backend = backend_for_target(target, &now, self.path_inputs.clone(), self.port);
        if let Some(existing) = self.load_backend()? {
            backend.created_at = existing.created_at;
        }
        self.register_os_backend(&mut backend)?;
        self.save_backend(&backend)?;
        Ok(backend)
    }

    pub(super) fn unregister_backend(&self) -> RefineResult<()> {
        if let Some(backend) = self.load_backend()?
            && let Some(path) = backend.service_metadata_path.clone()
        {
            let mut backend = backend;
            self.deactivate_os_backend(&mut backend);
            let path = PathBuf::from(path);
            if path.exists() {
                fs::remove_file(&path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to remove service metadata {}: {error}",
                        path.display()
                    ))
                })?;
            }
        }
        if self.backend_path().exists() {
            fs::remove_file(self.backend_path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove install backend {}: {error}",
                    self.backend_path().display()
                ))
            })?;
        }
        if let Some(legacy_backend_path) = self.legacy_backend_path()
            && legacy_backend_path.exists()
        {
            fs::remove_file(&legacy_backend_path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove legacy install backend {}: {error}",
                    legacy_backend_path.display()
                ))
            })?;
        }
        Ok(())
    }
}
