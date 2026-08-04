use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[cfg(not(test))]
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    DaemonReachability, daemon_executable_string, http_reachability_probe,
};
use crate::process::supervisor::runtime::{
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub service_manager: String,
    pub service_metadata_path: Option<String>,
    pub app_support_dir: Option<String>,
    pub cache_dir: Option<String>,
    pub logs_dir: Option<String>,
    pub credential_store: String,
    pub desktop_bundle: Option<String>,
    pub registered: bool,
    #[serde(default)]
    pub activated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activation_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deactivation_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_service_label: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub notes: Vec<String>,
}

pub trait InstallationService {
    fn install(&self, target: InstallTarget) -> RefineResult<InstallStatus>;
    fn repair(&self) -> RefineResult<InstallStatus>;
    fn record_metadata_update(&self, version: &str) -> RefineResult<InstallStatus>;
    fn rollback(&self) -> RefineResult<InstallStatus>;
    fn uninstall(&self) -> RefineResult<()>;
    fn status(&self) -> RefineResult<InstallStatus>;
    fn control_installed_service(
        &self,
        action: InstalledServiceAction,
    ) -> RefineResult<Option<ServiceManagerControl>>;
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
    pub port: Option<u16>,
    pub path_inputs: RuntimePathInputs,
}

mod backend_spec;
mod lifecycle;
mod os_backend;
mod service_control;
mod service_registration;
mod state;

use backend_spec::*;
pub use service_control::{InstalledServiceAction, ServiceManagerControl};
pub use service_registration::ServiceRegistrationUpdate;

#[cfg(test)]
mod tests;
