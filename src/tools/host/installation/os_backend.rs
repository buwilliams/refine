use super::*;

impl FileInstallationService {
    pub(super) fn register_os_backend(
        &self,
        backend: &mut InstallBackendRegistration,
    ) -> RefineResult<()> {
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

    pub(super) fn activate_os_backend(&self, backend: &mut InstallBackendRegistration) {
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

    pub(super) fn deactivate_os_backend(&self, backend: &mut InstallBackendRegistration) {
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

    pub(super) fn service_metadata(
        &self,
        backend: &InstallBackendRegistration,
    ) -> RefineResult<String> {
        match backend.target {
            InstallTarget::LinuxCliWeb => self.systemd_user_unit(backend),
            InstallTarget::MacOsAppBundle => self.launchd_plist(backend),
            InstallTarget::WindowsInstaller => self.windows_service_manifest(backend),
        }
    }

    pub(super) fn systemd_user_unit(
        &self,
        backend: &InstallBackendRegistration,
    ) -> RefineResult<String> {
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
            systemd_escape_path(backend.app_support_dir.as_deref().unwrap_or(".")),
            systemd_escape_path(logs_dir),
            systemd_escape_path(logs_dir)
        ))
    }

    pub(super) fn launchd_plist(
        &self,
        backend: &InstallBackendRegistration,
    ) -> RefineResult<String> {
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

    pub(super) fn windows_service_manifest(
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

    pub(super) fn default_target(&self) -> InstallTarget {
        match std::env::consts::OS {
            "macos" => InstallTarget::MacOsAppBundle,
            "windows" => InstallTarget::WindowsInstaller,
            _ => InstallTarget::LinuxCliWeb,
        }
    }

    pub(super) fn run_service_command(&self, command: &ServiceCommand) -> Result<(), String> {
        #[cfg(test)]
        {
            let _ = command;
            Ok(())
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
                    metadata: Default::default(),
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
