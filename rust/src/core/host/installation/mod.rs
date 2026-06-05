use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::runtime::{
    DEFAULT_APP_ID, RuntimeOs, RuntimePathInputs, RuntimePathLayout,
};

pub const INSTALL_STATE_FILE: &str = "install-state.json";
pub const INSTALL_BACKEND_FILE: &str = "install-backend.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallTarget {
    MacOsAppBundle,
    WindowsInstaller,
    LinuxCliWeb,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallStatus {
    pub installed: bool,
    pub target: InstallTarget,
    pub version: Option<String>,
    pub stale: bool,
    pub partial: bool,
    pub conflicting: bool,
    pub backend: Option<InstallBackendRegistration>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallBackendRegistration {
    pub target: InstallTarget,
    pub service_manager: String,
    pub service_metadata_path: Option<String>,
    pub app_support_dir: Option<String>,
    pub cache_dir: Option<String>,
    pub logs_dir: Option<String>,
    pub credential_store: String,
    pub desktop_bundle: Option<String>,
    pub registered: bool,
    pub created_at: String,
    pub updated_at: String,
    pub notes: Vec<String>,
}

pub trait InstallationService {
    fn install(&self, target: InstallTarget) -> RefineResult<InstallStatus>;
    fn repair(&self) -> RefineResult<InstallStatus>;
    fn update(&self, version: &str) -> RefineResult<InstallStatus>;
    fn rollback(&self) -> RefineResult<InstallStatus>;
    fn uninstall(&self) -> RefineResult<()>;
    fn status(&self) -> RefineResult<InstallStatus>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct InstallStateDocument {
    status: InstallStatus,
    previous_version: Option<String>,
    installed_at: Option<String>,
    updated_at: String,
}

#[derive(Clone, Debug)]
pub struct FileInstallationService {
    pub runtime_root: PathBuf,
    pub current_version: String,
}

impl FileInstallationService {
    pub fn new(runtime_root: impl Into<PathBuf>, current_version: impl Into<String>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            current_version: current_version.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.runtime_root.join(INSTALL_STATE_FILE)
    }

    pub fn backend_path(&self) -> PathBuf {
        self.runtime_root.join(INSTALL_BACKEND_FILE)
    }

    fn load(&self) -> RefineResult<InstallStateDocument> {
        let path = self.path();
        if !path.exists() {
            return Ok(default_state(self.default_target(), &self.current_version));
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

    fn save(&self, state: &InstallStateDocument) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
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
        })
    }

    fn load_backend(&self) -> RefineResult<Option<InstallBackendRegistration>> {
        let path = self.backend_path();
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

    fn save_backend(&self, backend: &InstallBackendRegistration) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
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
        })
    }

    fn register_backend(&self, target: InstallTarget) -> RefineResult<InstallBackendRegistration> {
        let now = now_timestamp();
        let mut backend = backend_for_target(target, &now);
        if let Some(existing) = self.load_backend()? {
            backend.created_at = existing.created_at;
        }
        self.save_backend(&backend)?;
        Ok(backend)
    }

    fn unregister_backend(&self) -> RefineResult<()> {
        if self.backend_path().exists() {
            fs::remove_file(self.backend_path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove install backend {}: {error}",
                    self.backend_path().display()
                ))
            })?;
        }
        Ok(())
    }

    fn default_target(&self) -> InstallTarget {
        match std::env::consts::OS {
            "macos" => InstallTarget::MacOsAppBundle,
            "windows" => InstallTarget::WindowsInstaller,
            _ => InstallTarget::LinuxCliWeb,
        }
    }
}

impl InstallationService for FileInstallationService {
    fn install(&self, target: InstallTarget) -> RefineResult<InstallStatus> {
        let now = now_timestamp();
        let backend = self.register_backend(target.clone())?;
        let state = InstallStateDocument {
            status: InstallStatus {
                installed: true,
                target,
                version: Some(self.current_version.clone()),
                stale: false,
                partial: false,
                conflicting: false,
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
        state.status.partial = false;
        state.status.conflicting = false;
        state.status.stale = false;
        state.status.backend = Some(self.register_backend(state.status.target.clone())?);
        if state.status.version.is_none() {
            state.status.version = Some(self.current_version.clone());
        }
        state.updated_at = now_timestamp();
        self.save(&state)?;
        Ok(state.status)
    }

    fn update(&self, version: &str) -> RefineResult<InstallStatus> {
        let version = version.trim();
        if version.is_empty() {
            return Err(RefineError::InvalidInput(
                "update version is required".to_string(),
            ));
        }
        let mut state = self.load()?;
        let backend = self.register_backend(state.status.target.clone())?;
        state.previous_version = state.status.version.clone();
        state.status.installed = true;
        state.status.version = Some(version.to_string());
        state.status.stale = false;
        state.status.partial = false;
        state.status.conflicting = false;
        state.status.backend = Some(backend);
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
        state.status.version = Some(previous);
        state.status.stale = false;
        state.status.partial = false;
        state.status.conflicting = false;
        state.status.backend = Some(self.register_backend(state.status.target.clone())?);
        state.previous_version = current;
        state.updated_at = now_timestamp();
        self.save(&state)?;
        Ok(state.status)
    }

    fn uninstall(&self) -> RefineResult<()> {
        let mut state = self.load()?;
        state.status.installed = false;
        state.status.stale = false;
        state.status.partial = false;
        state.status.conflicting = false;
        state.status.backend = None;
        state.updated_at = now_timestamp();
        self.unregister_backend()?;
        self.save(&state)
    }

    fn status(&self) -> RefineResult<InstallStatus> {
        let mut state = self.load()?;
        if state.status.installed
            && state.status.version.as_deref() != Some(self.current_version.as_str())
        {
            state.status.stale = true;
        }
        let backend = self.load_backend()?;
        state.status.partial = state.status.installed && backend.is_none();
        state.status.conflicting = state.status.installed
            && backend
                .as_ref()
                .map(|backend| backend.target != state.status.target || !backend.registered)
                .unwrap_or(false);
        state.status.backend = backend;
        Ok(state.status)
    }
}

fn default_state(target: InstallTarget, current_version: &str) -> InstallStateDocument {
    InstallStateDocument {
        status: InstallStatus {
            installed: false,
            target,
            version: Some(current_version.to_string()),
            stale: false,
            partial: false,
            conflicting: false,
            backend: None,
        },
        previous_version: None,
        installed_at: None,
        updated_at: now_timestamp(),
    }
}

fn backend_for_target(target: InstallTarget, timestamp: &str) -> InstallBackendRegistration {
    let (os, service_manager, credential_store, desktop_bundle, notes) = match target {
        InstallTarget::MacOsAppBundle => (
            RuntimeOs::Macos,
            "launchd_login_item",
            "keychain",
            Some("/Applications/Refine.app".to_string()),
            vec![
                "signed app bundle and notarization are represented by release packaging metadata"
                    .to_string(),
                "daemon auto-start uses launchd/Login Item registration".to_string(),
            ],
        ),
        InstallTarget::WindowsInstaller => (
            RuntimeOs::Windows,
            "windows_user_service",
            "windows_credential_manager",
            Some(r"%LOCALAPPDATA%\Programs\Refine\Refine.exe".to_string()),
            vec![
                "signed installer metadata is represented by release packaging metadata"
                    .to_string(),
                "daemon auto-start uses a user-session service strategy".to_string(),
            ],
        ),
        InstallTarget::LinuxCliWeb => (
            RuntimeOs::Linux,
            "systemd_user",
            "environment_or_provider_store",
            None,
            vec![
                "Linux install supports CLI/web with systemd user service when available"
                    .to_string(),
                "falls back to explicit process mode when systemd is unavailable".to_string(),
            ],
        ),
    };
    let layout = RuntimePathLayout::for_os(os, DEFAULT_APP_ID, RuntimePathInputs::from_env());
    InstallBackendRegistration {
        target,
        service_manager: service_manager.to_string(),
        service_metadata_path: layout
            .service_metadata_path
            .as_ref()
            .map(|path| path.display().to_string()),
        app_support_dir: Some(layout.app_support_dir.display().to_string()),
        cache_dir: Some(layout.cache_dir.display().to_string()),
        logs_dir: Some(layout.logs_dir.display().to_string()),
        credential_store: credential_store.to_string(),
        desktop_bundle,
        registered: true,
        created_at: timestamp.to_string(),
        updated_at: timestamp.to_string(),
        notes,
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_installation_service_persists_update_and_rollback_state() {
        let temp_root = unique_temp_dir("installation");
        let runtime_root = temp_root.join("run");
        let service = FileInstallationService::new(&runtime_root, "1.0.0");

        let initial = service.status().unwrap();
        assert!(!initial.installed);

        let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
        assert!(installed.installed);
        assert_eq!(installed.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            installed.backend.as_ref().unwrap().service_manager,
            "systemd_user"
        );
        assert!(service.path().exists());
        assert!(service.backend_path().exists());

        let updated = service.update("1.1.0").unwrap();
        assert_eq!(updated.version.as_deref(), Some("1.1.0"));
        assert_eq!(
            updated.backend.as_ref().unwrap().target,
            InstallTarget::LinuxCliWeb
        );
        let stale = FileInstallationService::new(&runtime_root, "1.2.0")
            .status()
            .unwrap();
        assert!(stale.stale);

        let rolled_back = service.rollback().unwrap();
        assert_eq!(rolled_back.version.as_deref(), Some("1.0.0"));

        service.uninstall().unwrap();
        assert!(!service.status().unwrap().installed);
        assert!(!service.backend_path().exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_installation_service_detects_partial_and_conflicting_backend_state() {
        let temp_root = unique_temp_dir("installation-backend");
        let runtime_root = temp_root.join("run");
        let service = FileInstallationService::new(&runtime_root, "1.0.0");

        service.install(InstallTarget::LinuxCliWeb).unwrap();
        fs::remove_file(service.backend_path()).unwrap();
        let partial = service.status().unwrap();
        assert!(partial.partial);
        assert!(!partial.conflicting);

        service.repair().unwrap();
        let mut backend = service.load_backend().unwrap().unwrap();
        backend.target = InstallTarget::WindowsInstaller;
        service.save_backend(&backend).unwrap();
        let conflicting = service.status().unwrap();
        assert!(conflicting.conflicting);

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
