use super::*;

impl FileInstallationService {
    pub fn new(runtime_root: impl Into<PathBuf>, current_version: impl Into<String>) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            checkout_root: checkout_root_for_runtime(&runtime_root),
            runtime_root,
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
        let runtime_root = runtime_root.into();
        Self {
            checkout_root: checkout_root_for_runtime(&runtime_root),
            runtime_root,
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
        let runtime_root = runtime_root.into();
        Self {
            checkout_root: checkout_root_for_runtime(&runtime_root),
            runtime_root,
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
        let runtime_root = runtime_root.into();
        Self {
            checkout_root: checkout_root_for_runtime(&runtime_root),
            runtime_root,
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
            && self.legacy_registration_belongs_to_selected_port()?
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
        let migrate_legacy = self.legacy_registration_belongs_to_selected_port()?;
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
            && (migrate_legacy
                || (self.backend_path().exists()
                    && self.legacy_backend_path().is_none_or(|path| !path.exists())))
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
            && self.legacy_registration_belongs_to_selected_port()?
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
        let migrate_legacy = self.legacy_registration_belongs_to_selected_port()?;
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
            && migrate_legacy
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
        repair_legacy: bool,
    ) -> RefineResult<InstallBackendRegistration> {
        self.require_installed_executable()?;
        let now = now_timestamp();
        let legacy_runtime_root = self.legacy_external_runtime_root(&target);
        if legacy_runtime_root.is_some() && !repair_legacy {
            return Err(RefineError::Conflict(format!(
                "legacy external Refine runtime detected at {}; it was not merged, deleted, or overwritten; inspect it, then run `./r system repair --port {}` from the intended checkout",
                legacy_runtime_root.as_deref().unwrap_or("unknown"),
                self.port.unwrap_or(8082)
            )));
        }
        let mut backend = backend_for_target(
            target,
            &now,
            self.path_inputs.clone(),
            self.port,
            &self.checkout_root,
        );
        if let Some(existing) = self.load_backend()? {
            backend.created_at = existing.created_at;
            backend.legacy_service_label = existing.legacy_service_label.or_else(|| {
                let path = existing.service_metadata_path?;
                let metadata = fs::read_to_string(path).ok()?;
                (existing.target == InstallTarget::MacosDaemon
                    && metadata.contains("<key>Label</key><string>com.refine.daemon</string>")
                    && service_control::launchd_label(&backend) != "com.refine.daemon")
                    .then(|| "com.refine.daemon".to_string())
            });
        }
        let legacy_registration = self.detect_external_registration(&mut backend, repair_legacy)?;
        if let Some(legacy_runtime_root) = legacy_runtime_root {
            backend.notes.push(format!(
                "legacy external runtime detected at {legacy_runtime_root}; explicit repair left it unchanged"
            ));
        }
        self.register_os_backend(&mut backend, legacy_registration.as_ref())?;
        self.save_backend(&backend)?;
        Ok(backend)
    }

    fn require_installed_executable(&self) -> RefineResult<()> {
        let executable = self.checkout_root.join("bin/refine");
        let metadata = fs::metadata(&executable).map_err(|error| {
            RefineError::NotFound(format!(
                "installed Refine mode requires an executable at {}; run `./r system install --port {}` from the owning checkout to build the release before registering or repairing a service: {error}",
                executable.display(),
                self.port.unwrap_or(8082),
            ))
        })?;
        if !metadata.is_file() {
            return Err(RefineError::NotFound(format!(
                "installed Refine mode requires a file at {}; run `./r system install --port {}` from the owning checkout to rebuild it before registering or repairing a service",
                executable.display(),
                self.port.unwrap_or(8082),
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(RefineError::InvalidInput(format!(
                    "installed Refine binary is not executable: {}; run `chmod +x {}` or reinstall it",
                    executable.display(),
                    executable.display()
                )));
            }
        }
        Ok(())
    }

    fn legacy_registration_belongs_to_selected_port(&self) -> RefineResult<bool> {
        const LEGACY_DEFAULT_PORT: u16 = 8082;
        let Some(selected_port) = self.port else {
            return Ok(true);
        };
        let Some(path) = self.legacy_backend_path() else {
            return Ok(false);
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read legacy install backend {}: {error}",
                    path.display()
                )));
            }
        };
        let backend =
            serde_json::from_slice::<InstallBackendRegistration>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse legacy install backend {}: {error}",
                    path.display()
                ))
            })?;
        if let Some(port) = backend.port {
            return Ok(port == selected_port);
        }
        let Some(metadata_path) = backend.service_metadata_path.as_deref() else {
            return Ok(false);
        };
        let metadata = match fs::read_to_string(metadata_path) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(false),
        };
        let explicit_ports = match backend.target {
            InstallTarget::LinuxCliWeb => systemd_exec_ports(&metadata),
            InstallTarget::MacosDaemon => launchd_program_ports(&metadata),
            InstallTarget::WindowsDaemon => Vec::new(),
        };
        if explicit_ports.contains(&selected_port) {
            return Ok(true);
        }
        let has_any_explicit_port = !explicit_ports.is_empty();
        Ok(selected_port == LEGACY_DEFAULT_PORT && !has_any_explicit_port)
    }

    pub(crate) fn uninstall_after_daemon_stopped(&self) -> RefineResult<()> {
        self.commit_uninstall(true)
    }

    pub(super) fn commit_uninstall(&self, daemon_already_stopped: bool) -> RefineResult<()> {
        let mut state = self.load()?;
        self.unregister_backend(daemon_already_stopped)?;
        state.status.installed = false;
        state.status.port = self.port;
        state.status.stale = false;
        state.status.partial = false;
        state.status.conflicting = false;
        state.status.backend = None;
        state.updated_at = now_timestamp();
        self.save(&state)
    }

    pub(super) fn unregister_backend(&self, daemon_already_stopped: bool) -> RefineResult<()> {
        let remove_legacy = self.legacy_registration_belongs_to_selected_port()?;
        let mut backend = self.load_backend()?;
        if let Some(backend) = backend.as_mut() {
            if daemon_already_stopped {
                self.deactivate_os_backend_after_stop(backend)?;
            } else {
                self.deactivate_os_backend(backend)?;
            }
        }
        if !daemon_already_stopped {
            self.confirm_uninstall_daemon_stopped(backend.as_ref())?;
        }
        if let Some(path) = backend.and_then(|backend| backend.service_metadata_path) {
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
            && remove_legacy
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

    fn confirm_uninstall_daemon_stopped(
        &self,
        backend: Option<&InstallBackendRegistration>,
    ) -> RefineResult<()> {
        let port = self
            .port
            .or_else(|| backend.and_then(|backend| backend.port))
            .ok_or_else(|| {
                RefineError::InvalidInput(
                    "uninstall requires a port so daemon shutdown can be confirmed".to_string(),
                )
            })?;
        match http_reachability_probe(port) {
            DaemonReachability::Unreachable(_) => Ok(()),
            DaemonReachability::Reachable => Err(RefineError::Degraded(format!(
                "refusing to remove the Refine installation because the daemon remains reachable on 127.0.0.1:{port}"
            ))),
            DaemonReachability::Unknown(error) => Err(RefineError::Degraded(format!(
                "refusing to remove the Refine installation because daemon shutdown on 127.0.0.1:{port} could not be confirmed: {error}"
            ))),
        }
    }
}

fn checkout_root_for_runtime(runtime_root: &std::path::Path) -> PathBuf {
    runtime_root
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| runtime_root.to_path_buf())
}

fn systemd_exec_ports(metadata: &str) -> Vec<u16> {
    metadata
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ExecStart="))
        .flat_map(parse_systemd_exec_arguments)
        .collect::<Vec<_>>()
        .windows(2)
        .filter_map(|pair| {
            (pair[0] == "--port")
                .then(|| pair[1].parse::<u16>().ok())
                .flatten()
        })
        .chain(
            metadata
                .lines()
                .filter_map(|line| line.trim().strip_prefix("ExecStart="))
                .flat_map(parse_systemd_exec_arguments)
                .filter_map(|argument| {
                    argument
                        .strip_prefix("--port=")
                        .and_then(|port| port.parse::<u16>().ok())
                }),
        )
        .collect()
}

pub(super) fn parse_systemd_exec_arguments(command: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                arguments.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    arguments
}

fn launchd_program_ports(metadata: &str) -> Vec<u16> {
    let arguments = metadata
        .split("<string>")
        .skip(1)
        .filter_map(|value| value.split_once("</string>").map(|(value, _)| value.trim()))
        .collect::<Vec<_>>();
    arguments
        .windows(2)
        .filter_map(|pair| {
            (pair[0] == "--port")
                .then(|| pair[1].parse::<u16>().ok())
                .flatten()
        })
        .collect()
}
