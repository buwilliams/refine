use super::*;

pub(super) fn redact_assignment(value: &str, key: &str) -> String {
    let marker = format!("{key}=");
    let mut output = String::new();
    let mut remaining = value;
    while let Some(index) = remaining.find(&marker) {
        let (before, after_before) = remaining.split_at(index);
        output.push_str(before);
        output.push_str(&marker);
        output.push_str("[redacted]");
        let after_value = &after_before[marker.len()..];
        if let Some(next_space) = after_value.find(char::is_whitespace) {
            remaining = &after_value[next_space..];
        } else {
            remaining = "";
        }
    }
    output.push_str(remaining);
    output
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(super) fn parse_allowed_commands(raw: &str) -> BTreeSet<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn detect_secret_backend() -> SecretStoreBackend {
    match std::env::consts::OS {
        "macos" if command_available("security") => SecretStoreBackend::MacosKeychain,
        "windows" if command_available("cmdkey") => SecretStoreBackend::WindowsCredentialManager,
        "linux" if command_available("secret-tool") => SecretStoreBackend::LinuxSecretService,
        _ => SecretStoreBackend::FileFallback,
    }
}

pub(super) fn command_available(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            dir.join(format!("{command}.exe")).is_file()
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

pub(super) fn validate_secret_id(value: &str) -> RefineResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RefineError::InvalidInput(
            "secret scope and name are required".to_string(),
        ));
    }
    if value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(RefineError::InvalidInput(
            "secret scope and name may only contain letters, numbers, dot, underscore, and hyphen"
                .to_string(),
        ));
    }
    Ok(())
}

pub(super) fn secret_storage_key(scope: &str, name: &str) -> RefineResult<String> {
    validate_secret_id(scope)?;
    validate_secret_id(name)?;
    Ok(format!("{scope}--{name}"))
}

pub(super) fn secret_service_name(scope: &str, name: &str) -> RefineResult<String> {
    Ok(format!("refine.{}", secret_storage_key(scope, name)?))
}

pub(super) fn write_secret_file(path: &Path, bytes: &[u8]) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create secret directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to open secret file {}: {error}",
            path.display()
        ))
    })?;
    file.write_all(bytes).map_err(|error| {
        RefineError::Io(format!(
            "failed to write secret file {}: {error}",
            path.display()
        ))
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostCommand {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

impl HostCommand {
    pub(super) fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub(super) fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub(super) fn authorization_command(&self) -> String {
        self.program.clone()
    }
}

pub(super) fn run_status(
    runtime_root: &Path,
    command: HostCommand,
    action: &str,
) -> RefineResult<()> {
    let output = run_managed_command(runtime_root, command, None, action)?;
    if output.success() {
        return Ok(());
    }
    Err(RefineError::Degraded(format!(
        "failed to {action}: {}",
        output.stderr.trim()
    )))
}

pub(super) fn run_output(
    runtime_root: &Path,
    command: HostCommand,
    action: &str,
) -> RefineResult<String> {
    let output = run_managed_command(runtime_root, command, None, action)?;
    if output.success() {
        return Ok(output.stdout.trim().to_string());
    }
    Err(RefineError::Degraded(format!(
        "failed to {action}: {}",
        output.stderr.trim()
    )))
}

pub(super) fn run_with_stdin(
    runtime_root: &Path,
    command: HostCommand,
    stdin_value: &str,
    action: &str,
) -> RefineResult<()> {
    let output = run_managed_command(runtime_root, command, Some(stdin_value.to_string()), action)?;
    if output.success() {
        return Ok(());
    }
    Err(RefineError::Degraded(format!(
        "failed to {action}: {}",
        output.stderr.trim()
    )))
}

pub(super) fn run_managed_command(
    runtime_root: &Path,
    command: HostCommand,
    stdin: Option<String>,
    action: &str,
) -> RefineResult<crate::infrastructure::process::subprocess::ManagedProcessOutput> {
    let authorization_command = command.authorization_command();
    FileProcessSupervisor::new(runtime_root)
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Maintenance,
            command: command.program,
            args: command.args,
            cwd: None,
            env: Vec::new(),
            stdin,
            limits: None,
            authorization_command: Some(authorization_command),
            sensitive: true,
            metadata: Default::default(),
        })
        .map_err(|error| RefineError::Io(format!("failed to {action}: {error}")))
}
