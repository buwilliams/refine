use super::*;

impl InstallationService for FileInstallationService {
    fn install(&self, target: InstallTarget) -> RefineResult<InstallStatus> {
        let now = now_timestamp();
        let backend = self.register_backend(target.clone(), false)?;
        let legacy_runtime_root = self.legacy_external_runtime_root(&target);
        let state = InstallStateDocument {
            status: InstallStatus {
                installed: true,
                port: self.port,
                target,
                version: Some(self.current_version.clone()),
                stale: false,
                partial: !backend_complete(&backend),
                conflicting: false,
                legacy_runtime_root,
                backend: Some(backend),
            },
            previous_version: None,
            installed_at: Some(now.clone()),
            updated_at: now,
        };
        self.save(&state)?;
        Ok(state.status)
    }

    fn repair(&self) -> RefineResult<InstallStatus> {
        let mut state = self.load()?;
        state.status.installed = true;
        state.status.port = self.port;
        let backend = self.register_backend(state.status.target.clone(), true)?;
        state.status.partial = !backend_complete(&backend);
        state.status.conflicting = false;
        state.status.stale = false;
        state.status.backend = Some(backend);
        state.status.legacy_runtime_root = self.legacy_external_runtime_root(&state.status.target);
        if state.status.version.is_none() {
            state.status.version = Some(self.current_version.clone());
        }
        state.updated_at = now_timestamp();
        self.save(&state)?;
        Ok(state.status)
    }

    fn record_metadata_update(&self, version: &str) -> RefineResult<InstallStatus> {
        let version = version.trim();
        if version.is_empty() {
            return Err(RefineError::InvalidInput(
                "update version is required".to_string(),
            ));
        }
        let mut state = self.load()?;
        let backend = self.register_backend(state.status.target.clone(), true)?;
        state.previous_version = state.status.version.clone();
        state.status.installed = true;
        state.status.port = self.port;
        state.status.version = Some(version.to_string());
        state.status.stale = false;
        state.status.partial = !backend_complete(&backend);
        state.status.conflicting = false;
        state.status.backend = Some(backend);
        state.status.legacy_runtime_root = self.legacy_external_runtime_root(&state.status.target);
        state.updated_at = now_timestamp();
        if state.installed_at.is_none() {
            state.installed_at = Some(state.updated_at.clone());
        }
        self.save(&state)?;
        Ok(state.status)
    }

    fn rollback(&self) -> RefineResult<InstallStatus> {
        let mut state = self.load()?;
        let Some(previous) = state.previous_version.clone() else {
            return Err(RefineError::Conflict(
                "no previous install version is available for rollback".to_string(),
            ));
        };
        let current = state.status.version.clone();
        state.status.installed = true;
        state.status.port = self.port;
        state.status.version = Some(previous);
        state.status.stale = false;
        let backend = self.register_backend(state.status.target.clone(), true)?;
        state.status.partial = !backend_complete(&backend);
        state.status.conflicting = false;
        state.status.backend = Some(backend);
        state.status.legacy_runtime_root = self.legacy_external_runtime_root(&state.status.target);
        state.previous_version = current;
        state.updated_at = now_timestamp();
        self.save(&state)?;
        Ok(state.status)
    }

    fn uninstall(&self) -> RefineResult<()> {
        self.commit_uninstall(false)
    }

    fn status(&self) -> RefineResult<InstallStatus> {
        let mut state = self.load()?;
        state.status.port = self.port;
        if state.status.installed
            && state.status.version.as_deref() != Some(self.current_version.as_str())
        {
            state.status.stale = true;
        }
        let backend = self.load_backend()?;
        state.status.partial = state.status.installed
            && backend
                .as_ref()
                .map(|backend| !backend_complete(backend))
                .unwrap_or(true);
        state.status.conflicting = state.status.installed
            && backend
                .as_ref()
                .map(|backend| backend.target != state.status.target)
                .unwrap_or(false);
        state.status.backend = backend;
        state.status.legacy_runtime_root = self.legacy_external_runtime_root(&state.status.target);
        state.status.conflicting |= state.status.legacy_runtime_root.is_some();
        Ok(state.status)
    }

    fn control_installed_service(
        &self,
        action: InstalledServiceAction,
    ) -> RefineResult<Option<ServiceManagerControl>> {
        FileInstallationService::control_installed_service(self, action)
    }
}
