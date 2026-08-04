use super::*;

pub(super) fn default_state(
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

pub(super) fn backend_for_target(
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
        legacy_service_label: None,
        created_at: timestamp.to_string(),
        updated_at: timestamp.to_string(),
        notes,
    }
}

pub(super) fn port_scoped_service_metadata_path(
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

pub(super) fn backend_complete(backend: &InstallBackendRegistration) -> bool {
    backend.registered && backend.activated
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ServiceCommand {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl ServiceCommand {
    pub(super) fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    pub(super) fn display(&self) -> String {
        let mut parts = vec![shell_word(&self.program)];
        parts.extend(self.args.iter().map(|arg| shell_word(arg)));
        parts.join(" ")
    }
}

pub(super) fn activation_commands(backend: &InstallBackendRegistration) -> Vec<ServiceCommand> {
    match backend.target {
        InstallTarget::LinuxCliWeb => {
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
                        service_control::systemd_unit_name(backend),
                    ],
                ),
            ]
        }
        InstallTarget::MacOsAppBundle => {
            let Some(plist) = backend.service_metadata_path.clone() else {
                return Vec::new();
            };
            let target = format!(
                "{}/{}",
                launchctl_gui_domain(),
                service_control::launchd_label(backend)
            );
            vec![
                ServiceCommand::new("launchctl", vec!["enable".to_string(), target]),
                ServiceCommand::new(
                    "launchctl",
                    vec!["bootstrap".to_string(), launchctl_gui_domain(), plist],
                ),
            ]
        }
        InstallTarget::WindowsInstaller => Vec::new(),
    }
}

pub(super) fn deactivation_commands(backend: &InstallBackendRegistration) -> Vec<ServiceCommand> {
    match backend.target {
        InstallTarget::LinuxCliWeb => {
            vec![
                ServiceCommand::new(
                    "systemctl",
                    vec![
                        "--user".to_string(),
                        "disable".to_string(),
                        "--now".to_string(),
                        service_control::systemd_unit_name(backend),
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
            let target = format!(
                "{}/{}",
                launchctl_gui_domain(),
                service_control::launchd_label(backend)
            );
            vec![
                ServiceCommand::new("launchctl", vec!["disable".to_string(), target]),
                ServiceCommand::new(
                    "launchctl",
                    vec!["bootout".to_string(), launchctl_gui_domain(), plist],
                ),
            ]
        }
        InstallTarget::WindowsInstaller => Vec::new(),
    }
}

/// Remove service-manager enablement after the daemon has already been stopped
/// and its non-reachability confirmed by the host lifecycle authority.
///
/// Repeating the full deactivation is not portable: launchd reports `bootout`
/// of an already stopped service as an error. These commands remove persistent
/// enablement without issuing a second stop.
pub(super) fn deactivation_after_stop_commands(
    backend: &InstallBackendRegistration,
) -> Vec<ServiceCommand> {
    match backend.target {
        InstallTarget::LinuxCliWeb => vec![
            ServiceCommand::new(
                "systemctl",
                vec![
                    "--user".to_string(),
                    "disable".to_string(),
                    service_control::systemd_unit_name(backend),
                ],
            ),
            ServiceCommand::new(
                "systemctl",
                vec!["--user".to_string(), "daemon-reload".to_string()],
            ),
        ],
        InstallTarget::MacOsAppBundle => vec![ServiceCommand::new(
            "launchctl",
            vec![
                "disable".to_string(),
                format!(
                    "{}/{}",
                    launchctl_gui_domain(),
                    service_control::launchd_label(backend)
                ),
            ],
        )],
        InstallTarget::WindowsInstaller => Vec::new(),
    }
}

#[cfg(target_family = "unix")]
pub(super) fn launchctl_gui_domain() -> String {
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
pub(super) fn launchctl_gui_domain() -> String {
    "gui/current".to_string()
}

/// Render one word of a systemd `ExecStart=` command line.
///
/// That line is split into words, so a value containing whitespace or shell-like
/// characters has to be quoted. Specifiers are expanded here, so a literal `%`
/// must be doubled, and inside quotes a backslash escapes the next character.
pub(super) fn systemd_escape_arg(value: &str) -> String {
    let value = systemd_escape_specifiers(value);
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '@'))
    {
        value
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Render a value for a systemd setting that takes a bare path, such as
/// `WorkingDirectory=` or a `StandardOutput=append:` target.
///
/// These settings consume the rest of the line as the path, so quoting is wrong
/// rather than merely unnecessary: systemd does not strip the quotes and rejects
/// the value as "path is not absolute". That broke every corporate home
/// directory, whose `@` tripped the `ExecStart=` quoting rules. Whitespace needs
/// no escaping either; only `%` does, because specifiers are still expanded.
pub(super) fn systemd_escape_path(value: &str) -> String {
    systemd_escape_specifiers(value)
}

/// Protect a literal `%` from systemd specifier expansion. An unescaped `%` is
/// either rejected (`Invalid slot`) or silently expanded into something else.
pub(super) fn systemd_escape_specifiers(value: &str) -> String {
    value.replace('%', "%%")
}

pub(super) fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
