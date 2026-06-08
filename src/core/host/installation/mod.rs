use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[cfg(not(test))]
use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::lifecycle::daemon_executable_string;
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

    fn state_root(&self) -> PathBuf {
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

    fn legacy_path(&self) -> Option<PathBuf> {
        self.port
            .map(|_| self.runtime_root.join(INSTALL_STATE_FILE))
    }

    fn legacy_backend_path(&self) -> Option<PathBuf> {
        self.port
            .map(|_| self.runtime_root.join(INSTALL_BACKEND_FILE))
    }

    fn load(&self) -> RefineResult<InstallStateDocument> {
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

    fn save(&self, state: &InstallStateDocument) -> RefineResult<()> {
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

    fn load_backend(&self) -> RefineResult<Option<InstallBackendRegistration>> {
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

    fn save_backend(&self, backend: &InstallBackendRegistration) -> RefineResult<()> {
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

    fn register_backend(&self, target: InstallTarget) -> RefineResult<InstallBackendRegistration> {
        let now = now_timestamp();
        let mut backend = backend_for_target(target, &now, self.path_inputs.clone(), self.port);
        if let Some(existing) = self.load_backend()? {
            backend.created_at = existing.created_at;
        }
        self.register_os_backend(&mut backend)?;
        self.save_backend(&backend)?;
        Ok(backend)
    }

    fn unregister_backend(&self) -> RefineResult<()> {
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

    fn register_os_backend(&self, backend: &mut InstallBackendRegistration) -> RefineResult<()> {
        if let Some(path) = &backend.service_metadata_path {
            let path = PathBuf::from(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to create service metadata directory {}: {error}",
                        parent.display()
                    ))
                })?;
            }
            if let Some(app_support_dir) = &backend.app_support_dir {
                fs::create_dir_all(app_support_dir).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to create app support directory {app_support_dir}: {error}"
                    ))
                })?;
            }
            if let Some(cache_dir) = &backend.cache_dir {
                fs::create_dir_all(cache_dir).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to create cache directory {cache_dir}: {error}"
                    ))
                })?;
            }
            if let Some(logs_dir) = &backend.logs_dir {
                fs::create_dir_all(logs_dir).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to create logs directory {logs_dir}: {error}"
                    ))
                })?;
            }
            let metadata = self.service_metadata(backend)?;
            fs::write(&path, metadata).map_err(|error| {
                RefineError::Io(format!(
                    "failed to write service metadata {}: {error}",
                    path.display()
                ))
            })?;
            backend.registered = true;
            backend.notes.push(format!(
                "native service metadata written to {}",
                path.display()
            ));
            self.activate_os_backend(backend);
        } else {
            backend.registered = false;
            backend.activated = false;
            backend
                .notes
                .push("no native service metadata path is available on this platform".to_string());
        }
        backend.updated_at = now_timestamp();
        Ok(())
    }

    fn activate_os_backend(&self, backend: &mut InstallBackendRegistration) {
        backend.activation_error = None;
        let commands = activation_commands(backend);
        backend.activation_commands = commands.iter().map(ServiceCommand::display).collect();
        if commands.is_empty() {
            backend.activated = false;
            backend
                .notes
                .push("service activation is handled by the platform installer".to_string());
            return;
        }
        for command in commands {
            if let Err(error) = self.run_service_command(&command) {
                backend.activated = false;
                backend.activation_error = Some(error.clone());
                backend.notes.push(format!(
                    "native service activation failed while running `{}`: {error}",
                    command.display()
                ));
                return;
            }
        }
        backend.activated = true;
        backend
            .notes
            .push("native service activated with the platform service manager".to_string());
    }

    fn deactivate_os_backend(&self, backend: &mut InstallBackendRegistration) {
        let commands = deactivation_commands(backend);
        backend.deactivation_commands = commands.iter().map(ServiceCommand::display).collect();
        for command in commands {
            if let Err(error) = self.run_service_command(&command) {
                backend.notes.push(format!(
                    "native service deactivation failed while running `{}`: {error}",
                    command.display()
                ));
                return;
            }
        }
        if !backend.deactivation_commands.is_empty() {
            backend.activated = false;
        }
    }

    fn service_metadata(&self, backend: &InstallBackendRegistration) -> RefineResult<String> {
        match backend.target {
            InstallTarget::LinuxCliWeb => self.systemd_user_unit(backend),
            InstallTarget::MacOsAppBundle => self.launchd_plist(backend),
            InstallTarget::WindowsInstaller => self.windows_service_manifest(backend),
        }
    }

    fn systemd_user_unit(&self, backend: &InstallBackendRegistration) -> RefineResult<String> {
        let exe = daemon_executable_string()?;
        let logs_dir = backend.logs_dir.as_deref().unwrap_or(".");
        let port_args = backend
            .port
            .map(|port| format!(" --port {}", systemd_escape_arg(&port.to_string())))
            .unwrap_or_default();
        Ok(format!(
            "[Unit]\nDescription=Refine daemon\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={} system start --foreground{} --runtime-root {}\nRestart=on-failure\nRestartSec=3\nWorkingDirectory={}\nStandardOutput=append:{}/daemon.log\nStandardError=append:{}/daemon.err.log\n\n[Install]\nWantedBy=default.target\n",
            systemd_escape_arg(&exe),
            port_args,
            systemd_escape_arg(&self.runtime_root.display().to_string()),
            systemd_escape_arg(backend.app_support_dir.as_deref().unwrap_or(".")),
            logs_dir,
            logs_dir
        ))
    }

    fn launchd_plist(&self, backend: &InstallBackendRegistration) -> RefineResult<String> {
        let exe = xml_escape(&daemon_executable_string()?);
        let runtime_root = xml_escape(&self.runtime_root.display().to_string());
        let logs_dir = xml_escape(backend.logs_dir.as_deref().unwrap_or("."));
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.refine.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>system</string>
    <string>start</string>
    <string>--foreground</string>
    <string>--runtime-root</string>
    <string>{runtime_root}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{logs_dir}/daemon.log</string>
  <key>StandardErrorPath</key><string>{logs_dir}/daemon.err.log</string>
</dict>
</plist>
"#
        ))
    }

    fn windows_service_manifest(
        &self,
        backend: &InstallBackendRegistration,
    ) -> RefineResult<String> {
        let manifest = serde_json::json!({
            "service_name": "Refine",
            "display_name": "Refine daemon",
            "executable": daemon_executable_string()?,
            "arguments": ["system", "start", "--foreground", "--runtime-root", self.runtime_root.display().to_string()],
            "app_support_dir": backend.app_support_dir,
            "logs_dir": backend.logs_dir,
            "notes": "Windows service creation is represented as installer metadata; installer should register this manifest with the service manager."
        });
        serde_json::to_string_pretty(&manifest).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode Windows service manifest: {error}"
            ))
        })
    }

    fn default_target(&self) -> InstallTarget {
        match std::env::consts::OS {
            "macos" => InstallTarget::MacOsAppBundle,
            "windows" => InstallTarget::WindowsInstaller,
            _ => InstallTarget::LinuxCliWeb,
        }
    }

    fn run_service_command(&self, command: &ServiceCommand) -> Result<(), String> {
        #[cfg(test)]
        {
            let _ = command;
            return Ok(());
        }

        #[cfg(not(test))]
        {
            let output = FileProcessSupervisor::new(&self.runtime_root)
                .run_to_completion(ManagedProcessSpec {
                    owner: ProcessOwner::Maintenance,
                    command: command.program.clone(),
                    args: command.args.clone(),
                    cwd: None,
                    env: Vec::new(),
                    stdin: None,
                    limits: None,
                    authorization_command: Some(command.display()),
                    sensitive: false,
                })
                .map_err(|error| error.to_string())?;
            if output.success() {
                return Ok(());
            }
            let stderr = output.stderr.trim().to_string();
            let stdout = output.stdout.trim().to_string();
            let detail = if stderr.is_empty() { stdout } else { stderr };
            if detail.is_empty() {
                Err(format!(
                    "exited with {}",
                    output
                        .process
                        .exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ))
            } else {
                Err(detail)
            }
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
                port: self.port,
                target,
                version: Some(self.current_version.clone()),
                stale: false,
                partial: !backend_complete(&backend),
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
        state.status.port = self.port;
        let backend = self.register_backend(state.status.target.clone())?;
        state.status.partial = !backend_complete(&backend);
        state.status.conflicting = false;
        state.status.stale = false;
        state.status.backend = Some(backend);
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
        let backend = self.register_backend(state.status.target.clone())?;
        state.previous_version = state.status.version.clone();
        state.status.installed = true;
        state.status.port = self.port;
        state.status.version = Some(version.to_string());
        state.status.stale = false;
        state.status.partial = !backend_complete(&backend);
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
        state.status.port = self.port;
        state.status.version = Some(previous);
        state.status.stale = false;
        let backend = self.register_backend(state.status.target.clone())?;
        state.status.partial = !backend_complete(&backend);
        state.status.conflicting = false;
        state.status.backend = Some(backend);
        state.previous_version = current;
        state.updated_at = now_timestamp();
        self.save(&state)?;
        Ok(state.status)
    }

    fn uninstall(&self) -> RefineResult<()> {
        let mut state = self.load()?;
        state.status.installed = false;
        state.status.port = self.port;
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
        Ok(state.status)
    }
}

fn default_state(
    target: InstallTarget,
    current_version: &str,
    port: Option<u16>,
) -> InstallStateDocument {
    InstallStateDocument {
        status: InstallStatus {
            installed: false,
            port,
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

fn backend_for_target(
    target: InstallTarget,
    timestamp: &str,
    path_inputs: RuntimePathInputs,
    port: Option<u16>,
) -> InstallBackendRegistration {
    let (os, service_manager, credential_store, desktop_bundle, notes) = match &target {
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
    let layout = RuntimePathLayout::for_os(os, DEFAULT_APP_ID, path_inputs);
    let service_metadata_path = layout
        .service_metadata_path
        .as_ref()
        .map(|path| port_scoped_service_metadata_path(path, &target, port))
        .map(|path| path.display().to_string());
    InstallBackendRegistration {
        target,
        port,
        service_manager: service_manager.to_string(),
        service_metadata_path,
        app_support_dir: Some(layout.app_support_dir.display().to_string()),
        cache_dir: Some(layout.cache_dir.display().to_string()),
        logs_dir: Some(layout.logs_dir.display().to_string()),
        credential_store: credential_store.to_string(),
        desktop_bundle,
        registered: false,
        activated: false,
        activation_commands: Vec::new(),
        deactivation_commands: Vec::new(),
        activation_error: None,
        created_at: timestamp.to_string(),
        updated_at: timestamp.to_string(),
        notes,
    }
}

fn port_scoped_service_metadata_path(
    path: &std::path::Path,
    target: &InstallTarget,
    port: Option<u16>,
) -> PathBuf {
    let Some(port) = port else {
        return path.to_path_buf();
    };
    let file_name = match target {
        InstallTarget::LinuxCliWeb => format!("refine-{port}.service"),
        InstallTarget::MacOsAppBundle => format!("com.refine.daemon-{port}.plist"),
        InstallTarget::WindowsInstaller => format!("service-{port}.json"),
    };
    path.with_file_name(file_name)
}

fn backend_complete(backend: &InstallBackendRegistration) -> bool {
    backend.registered && backend.activated
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServiceCommand {
    program: String,
    args: Vec<String>,
}

impl ServiceCommand {
    fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    fn display(&self) -> String {
        let mut parts = vec![shell_word(&self.program)];
        parts.extend(self.args.iter().map(|arg| shell_word(arg)));
        parts.join(" ")
    }
}

fn activation_commands(backend: &InstallBackendRegistration) -> Vec<ServiceCommand> {
    match backend.target {
        InstallTarget::LinuxCliWeb => {
            let unit = backend
                .service_metadata_path
                .as_deref()
                .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_owned()))
                .and_then(|name| name.to_str().map(str::to_string))
                .unwrap_or_else(|| "refine.service".to_string());
            vec![
                ServiceCommand::new(
                    "systemctl",
                    vec!["--user".to_string(), "daemon-reload".to_string()],
                ),
                ServiceCommand::new(
                    "systemctl",
                    vec![
                        "--user".to_string(),
                        "enable".to_string(),
                        "--now".to_string(),
                        unit,
                    ],
                ),
            ]
        }
        InstallTarget::MacOsAppBundle => {
            let Some(plist) = backend.service_metadata_path.clone() else {
                return Vec::new();
            };
            let domain = launchctl_gui_domain();
            vec![
                ServiceCommand::new(
                    "launchctl",
                    vec!["bootstrap".to_string(), domain.clone(), plist],
                ),
                ServiceCommand::new(
                    "launchctl",
                    vec!["enable".to_string(), format!("{domain}/com.refine.daemon")],
                ),
            ]
        }
        InstallTarget::WindowsInstaller => Vec::new(),
    }
}

fn deactivation_commands(backend: &InstallBackendRegistration) -> Vec<ServiceCommand> {
    match backend.target {
        InstallTarget::LinuxCliWeb => {
            let unit = backend
                .service_metadata_path
                .as_deref()
                .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_owned()))
                .and_then(|name| name.to_str().map(str::to_string))
                .unwrap_or_else(|| "refine.service".to_string());
            vec![
                ServiceCommand::new(
                    "systemctl",
                    vec![
                        "--user".to_string(),
                        "disable".to_string(),
                        "--now".to_string(),
                        unit,
                    ],
                ),
                ServiceCommand::new(
                    "systemctl",
                    vec!["--user".to_string(), "daemon-reload".to_string()],
                ),
            ]
        }
        InstallTarget::MacOsAppBundle => {
            let Some(plist) = backend.service_metadata_path.clone() else {
                return Vec::new();
            };
            vec![ServiceCommand::new(
                "launchctl",
                vec!["bootout".to_string(), launchctl_gui_domain(), plist],
            )]
        }
        InstallTarget::WindowsInstaller => Vec::new(),
    }
}

#[cfg(target_family = "unix")]
fn launchctl_gui_domain() -> String {
    format!("gui/{}", unsafe { libc_getuid() })
}

#[cfg(target_family = "unix")]
unsafe fn libc_getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(not(target_family = "unix"))]
fn launchctl_gui_domain() -> String {
    "gui/current".to_string()
}

fn systemd_escape_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn file_installation_service_persists_update_and_rollback_state() {
        let temp_root = unique_temp_dir("installation");
        let runtime_root = temp_root.join("run");
        let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4557, &temp_root);

        let initial = service.status().unwrap();
        assert!(!initial.installed);
        assert_eq!(initial.port, Some(4557));

        let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
        assert!(installed.installed);
        assert_eq!(installed.port, Some(4557));
        assert!(!installed.partial);
        assert_eq!(installed.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            installed.backend.as_ref().unwrap().service_manager,
            "systemd_user"
        );
        assert!(installed.backend.as_ref().unwrap().registered);
        assert!(installed.backend.as_ref().unwrap().activated);
        assert!(
            installed
                .backend
                .as_ref()
                .unwrap()
                .activation_commands
                .iter()
                .any(|command| command.contains("'systemctl' '--user' 'enable' '--now'"))
        );
        let service_metadata_path = PathBuf::from(
            installed
                .backend
                .as_ref()
                .unwrap()
                .service_metadata_path
                .as_ref()
                .unwrap(),
        );
        assert_eq!(
            service_metadata_path.file_name().unwrap().to_str().unwrap(),
            "refine-4557.service"
        );
        assert!(service_metadata_path.exists());
        let unit = fs::read_to_string(&service_metadata_path).unwrap();
        assert!(unit.contains("ExecStart="));
        assert!(unit.contains("system start --foreground"));
        assert!(unit.contains("--port 4557 --runtime-root"));
        assert!(service.path().exists());
        assert!(service.backend_path().exists());
        assert_eq!(
            service.path(),
            runtime_root.join("4557").join(INSTALL_STATE_FILE)
        );
        assert_eq!(
            service.backend_path(),
            runtime_root.join("4557").join(INSTALL_BACKEND_FILE)
        );

        let updated = service.record_metadata_update("1.1.0").unwrap();
        assert_eq!(updated.version.as_deref(), Some("1.1.0"));
        assert_eq!(
            updated.backend.as_ref().unwrap().target,
            InstallTarget::LinuxCliWeb
        );
        let stale = test_installation_service_for_port(&runtime_root, "1.2.0", 4557, &temp_root)
            .status()
            .unwrap();
        assert!(stale.stale);

        let rolled_back = service.rollback().unwrap();
        assert_eq!(rolled_back.version.as_deref(), Some("1.0.0"));

        service.uninstall().unwrap();
        assert!(!service.status().unwrap().installed);
        assert!(!service.backend_path().exists());
        assert!(!service_metadata_path.exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn service_metadata_uses_deployed_binary_executable_when_launched_from_wrapper() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_mode = std::env::var("REFINE_LAUNCH_MODE").ok();
        let old_executable = std::env::var("REFINE_LAUNCH_EXECUTABLE").ok();
        unsafe {
            std::env::set_var("REFINE_LAUNCH_MODE", "binary");
            std::env::set_var("REFINE_LAUNCH_EXECUTABLE", "/opt/refine/bin/refine");
        }

        let temp_root = unique_temp_dir("installation-release-bin");
        let runtime_root = temp_root.join("run");
        let service = test_installation_service_for_port(&runtime_root, "1.0.0", 8082, &temp_root);

        let installed = service.install(InstallTarget::LinuxCliWeb).unwrap();
        let service_metadata_path = PathBuf::from(
            installed
                .backend
                .as_ref()
                .unwrap()
                .service_metadata_path
                .as_ref()
                .unwrap(),
        );
        let unit = fs::read_to_string(&service_metadata_path).unwrap();
        assert!(unit.contains(
            "ExecStart=/opt/refine/bin/refine system start --foreground --port 8082 --runtime-root"
        ));

        restore_env("REFINE_LAUNCH_MODE", old_mode);
        restore_env("REFINE_LAUNCH_EXECUTABLE", old_executable);
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn uninstall_is_scoped_to_selected_port() {
        let temp_root = unique_temp_dir("installation-port-scope");
        let runtime_root = temp_root.join("run");
        let first = test_installation_service_for_port(&runtime_root, "1.0.0", 8081, &temp_root);
        let second = test_installation_service_for_port(&runtime_root, "1.0.0", 8082, &temp_root);

        let first_metadata = PathBuf::from(
            first
                .install(InstallTarget::LinuxCliWeb)
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .service_metadata_path
                .as_ref()
                .unwrap(),
        );
        let second_metadata = PathBuf::from(
            second
                .install(InstallTarget::LinuxCliWeb)
                .unwrap()
                .backend
                .as_ref()
                .unwrap()
                .service_metadata_path
                .as_ref()
                .unwrap(),
        );

        first.uninstall().unwrap();

        assert!(!first.backend_path().exists());
        assert!(!first_metadata.exists());
        assert!(second.path().exists());
        assert!(second.backend_path().exists());
        assert!(second_metadata.exists());
        assert!(second.status().unwrap().installed);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn port_scoped_repair_can_migrate_legacy_root_install_state() {
        let temp_root = unique_temp_dir("installation-legacy-port-migration");
        let runtime_root = temp_root.join("run");
        let legacy = test_installation_service(&runtime_root, "1.0.0", &temp_root);
        let scoped = test_installation_service_for_port(&runtime_root, "1.1.0", 8080, &temp_root);

        legacy.install(InstallTarget::LinuxCliWeb).unwrap();
        assert!(runtime_root.join(INSTALL_STATE_FILE).exists());
        assert!(!scoped.path().exists());

        let repaired = scoped.repair().unwrap();

        assert_eq!(repaired.port, Some(8080));
        assert_eq!(repaired.version.as_deref(), Some("1.0.0"));
        assert!(scoped.path().exists());
        assert!(scoped.backend_path().exists());
        assert!(!runtime_root.join(INSTALL_STATE_FILE).exists());
        assert!(!runtime_root.join(INSTALL_BACKEND_FILE).exists());
        let unit = fs::read_to_string(
            repaired
                .backend
                .as_ref()
                .unwrap()
                .service_metadata_path
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert!(unit.contains("--port 8080 --runtime-root"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_installation_service_detects_partial_and_conflicting_backend_state() {
        let temp_root = unique_temp_dir("installation-backend");
        let runtime_root = temp_root.join("run");
        let service = test_installation_service_for_port(&runtime_root, "1.0.0", 4558, &temp_root);

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

    fn test_installation_service_for_port(
        runtime_root: &PathBuf,
        version: &str,
        port: u16,
        temp_root: &Path,
    ) -> FileInstallationService {
        FileInstallationService::with_path_inputs_for_port(
            runtime_root,
            version,
            port,
            RuntimePathInputs {
                home: Some(temp_root.join("home")),
                local_app_data: Some(temp_root.join("local-app-data")),
                app_data: Some(temp_root.join("app-data")),
                program_data: Some(temp_root.join("program-data")),
                xdg_cache_home: Some(temp_root.join("cache")),
                xdg_state_home: Some(temp_root.join("state")),
                xdg_config_home: Some(temp_root.join("config")),
            },
        )
    }

    fn test_installation_service(
        runtime_root: &PathBuf,
        version: &str,
        temp_root: &Path,
    ) -> FileInstallationService {
        FileInstallationService::with_path_inputs(
            runtime_root,
            version,
            RuntimePathInputs {
                home: Some(temp_root.join("home")),
                local_app_data: Some(temp_root.join("local-app-data")),
                app_data: Some(temp_root.join("app-data")),
                program_data: Some(temp_root.join("program-data")),
                xdg_cache_home: Some(temp_root.join("cache")),
                xdg_state_home: Some(temp_root.join("state")),
                xdg_config_home: Some(temp_root.join("config")),
            },
        )
    }

    fn restore_env(key: &str, value: Option<String>) {
        unsafe {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
