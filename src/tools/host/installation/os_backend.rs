use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ServiceCommandOutput {
    pub(super) exit_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

impl ServiceCommandOutput {
    #[cfg(test)]
    pub(super) fn success() -> Self {
        Self {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn exited(
        exit_code: i32,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Self {
        Self {
            exit_code: Some(exit_code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    pub(super) fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }

    pub(super) fn failure_detail(&self) -> String {
        let stderr = self.stderr.trim();
        let stdout = self.stdout.trim();
        if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!(
                "exited with {}",
                self.exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown status".to_string())
            )
        }
    }

    pub(super) fn require_success(self) -> Result<(), String> {
        if self.succeeded() {
            Ok(())
        } else {
            Err(self.failure_detail())
        }
    }
}

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
        let executable = PathBuf::from(daemon_executable_string()?);
        self.service_metadata_for_executable(backend, &executable)
    }

    pub(super) fn service_metadata_for_executable(
        &self,
        backend: &InstallBackendRegistration,
        executable: &std::path::Path,
    ) -> RefineResult<String> {
        match backend.target {
            InstallTarget::LinuxCliWeb => self.systemd_user_unit(backend, executable),
            InstallTarget::MacOsAppBundle => self.launchd_plist(backend, executable),
            InstallTarget::WindowsInstaller => self.windows_service_manifest(backend, executable),
        }
    }

    pub(super) fn systemd_user_unit(
        &self,
        backend: &InstallBackendRegistration,
        executable: &std::path::Path,
    ) -> RefineResult<String> {
        let exe = executable.display().to_string();
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
        executable: &std::path::Path,
    ) -> RefineResult<String> {
        let exe = xml_escape(&executable.display().to_string());
        let runtime_root = xml_escape(&self.runtime_root.display().to_string());
        let logs_dir = xml_escape(backend.logs_dir.as_deref().unwrap_or("."));
        let label = xml_escape(&service_control::launchd_label(backend));
        let port_arguments = backend
            .port
            .map(|port| {
                format!(
                    "    <string>--port</string>\n    <string>{}</string>\n",
                    xml_escape(&port.to_string())
                )
            })
            .unwrap_or_default();
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>system</string>
    <string>start</string>
    <string>--foreground</string>
{port_arguments}    <string>--runtime-root</string>
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
        executable: &std::path::Path,
    ) -> RefineResult<String> {
        let manifest = serde_json::json!({
            "service_name": "Refine",
            "display_name": "Refine daemon",
            "executable": executable.display().to_string(),
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
        self.execute_service_command(command)?.require_success()
    }

    pub(super) fn execute_service_command(
        &self,
        command: &ServiceCommand,
    ) -> Result<ServiceCommandOutput, String> {
        #[cfg(test)]
        {
            let _ = command;
            Ok(ServiceCommandOutput::success())
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
            Ok(ServiceCommandOutput {
                exit_code: output.process.exit_code,
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
    }
}
