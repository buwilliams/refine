use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use crate::core::supervisor::config::{ConfigService, FileSettingsService};
use crate::core::supervisor::errors::{RefineError, RefineResult};

pub const SECURITY_AUDIT_FILE: &str = "security-audit.jsonl";
pub const SECRET_INDEX_FILE: &str = "secret-index.json";

pub trait SecurityService {
    fn redact(&self, value: &str) -> String;
    fn audit(&self, actor: &str, command: &str) -> RefineResult<()>;
}

pub trait SecretStore {
    fn backend_status(&self) -> SecretStoreStatus;
    fn put_secret(&self, scope: &str, name: &str, value: &str) -> RefineResult<SecretMetadata>;
    fn get_secret(&self, scope: &str, name: &str) -> RefineResult<SecretValue>;
    fn delete_secret(&self, scope: &str, name: &str) -> RefineResult<SecretMetadata>;
    fn list_secrets(&self) -> RefineResult<Vec<SecretMetadata>>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStoreBackend {
    MacosKeychain,
    WindowsCredentialManager,
    LinuxSecretService,
    FileFallback,
}

impl SecretStoreBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MacosKeychain => "macos_keychain",
            Self::WindowsCredentialManager => "windows_credential_manager",
            Self::LinuxSecretService => "linux_secret_service",
            Self::FileFallback => "file_fallback",
        }
    }

    pub fn native(&self) -> bool {
        !matches!(self, Self::FileFallback)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecretStoreStatus {
    pub backend: SecretStoreBackend,
    pub native: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecretMetadata {
    pub scope: String,
    pub name: String,
    pub backend: SecretStoreBackend,
    pub native: bool,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecretValue {
    pub metadata: SecretMetadata,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct SecretEnvelope {
    value: String,
    updated_at: String,
}

#[derive(Clone, Debug)]
pub struct NativeSecretStore {
    pub runtime_root: PathBuf,
    pub preferred_backend: SecretStoreBackend,
}

impl NativeSecretStore {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            preferred_backend: detect_secret_backend(),
        }
    }

    pub fn with_backend(
        runtime_root: impl Into<PathBuf>,
        preferred_backend: SecretStoreBackend,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            preferred_backend,
        }
    }

    fn secrets_dir(&self) -> PathBuf {
        self.runtime_root.join("secrets")
    }

    fn values_dir(&self) -> PathBuf {
        self.secrets_dir().join("values")
    }

    fn index_path(&self) -> PathBuf {
        self.secrets_dir().join(SECRET_INDEX_FILE)
    }

    fn file_secret_path(&self, scope: &str, name: &str) -> RefineResult<PathBuf> {
        Ok(self
            .values_dir()
            .join(format!("{}.json", secret_storage_key(scope, name)?)))
    }

    fn load_index(&self) -> RefineResult<Vec<SecretMetadata>> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read secret index {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<Vec<SecretMetadata>>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse secret index {}: {error}",
                path.display()
            ))
        })
    }

    fn save_index(&self, index: &[SecretMetadata]) -> RefineResult<()> {
        fs::create_dir_all(self.secrets_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create secret directory {}: {error}",
                self.secrets_dir().display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(index).map_err(|error| {
            RefineError::Serialization(format!("failed to encode secret index: {error}"))
        })?;
        fs::write(self.index_path(), encoded)
            .map_err(|error| RefineError::Io(format!("failed to write secret index: {error}")))
    }

    fn upsert_index(&self, metadata: SecretMetadata) -> RefineResult<SecretMetadata> {
        let mut index = self.load_index()?;
        index.retain(|entry| entry.scope != metadata.scope || entry.name != metadata.name);
        index.push(metadata.clone());
        index.sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
        self.save_index(&index)?;
        Ok(metadata)
    }

    fn remove_from_index(&self, scope: &str, name: &str) -> RefineResult<SecretMetadata> {
        let mut index = self.load_index()?;
        let Some(position) = index
            .iter()
            .position(|entry| entry.scope == scope && entry.name == name)
        else {
            return Err(RefineError::NotFound(format!(
                "secret {scope}/{name} was not found"
            )));
        };
        let metadata = index.remove(position);
        self.save_index(&index)?;
        Ok(metadata)
    }

    fn put_file_secret(
        &self,
        scope: &str,
        name: &str,
        value: &str,
    ) -> RefineResult<SecretMetadata> {
        fs::create_dir_all(self.values_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create secret values directory {}: {error}",
                self.values_dir().display()
            ))
        })?;
        let updated_at = now_timestamp();
        let envelope = SecretEnvelope {
            value: value.to_string(),
            updated_at: updated_at.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&envelope).map_err(|error| {
            RefineError::Serialization(format!("failed to encode secret value: {error}"))
        })?;
        let path = self.file_secret_path(scope, name)?;
        write_secret_file(&path, &encoded)?;
        self.upsert_index(SecretMetadata {
            scope: scope.to_string(),
            name: name.to_string(),
            backend: SecretStoreBackend::FileFallback,
            native: false,
            updated_at,
        })
    }

    fn get_file_secret(&self, scope: &str, name: &str) -> RefineResult<SecretValue> {
        let path = self.file_secret_path(scope, name)?;
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!("failed to read secret {}: {error}", path.display()))
        })?;
        let envelope = serde_json::from_slice::<SecretEnvelope>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse secret {}: {error}",
                path.display()
            ))
        })?;
        Ok(SecretValue {
            metadata: SecretMetadata {
                scope: scope.to_string(),
                name: name.to_string(),
                backend: SecretStoreBackend::FileFallback,
                native: false,
                updated_at: envelope.updated_at,
            },
            value: envelope.value,
        })
    }

    fn delete_file_secret(&self, scope: &str, name: &str) -> RefineResult<()> {
        let path = self.file_secret_path(scope, name)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to delete secret {}: {error}",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    fn put_native_secret(
        &self,
        backend: &SecretStoreBackend,
        scope: &str,
        name: &str,
        value: &str,
    ) -> RefineResult<()> {
        match backend {
            SecretStoreBackend::MacosKeychain => run_status(
                &self.runtime_root,
                HostCommand::new("security")
                    .arg("add-generic-password")
                    .arg("-U")
                    .arg("-s")
                    .arg(secret_service_name(scope, name)?)
                    .arg("-a")
                    .arg(scope)
                    .arg("-w")
                    .arg(value),
                "store secret in macOS Keychain",
            ),
            SecretStoreBackend::LinuxSecretService => run_with_stdin(
                &self.runtime_root,
                HostCommand::new("secret-tool")
                    .arg("store")
                    .arg("--label")
                    .arg(format!("Refine {scope}/{name}"))
                    .arg("application")
                    .arg("refine")
                    .arg("scope")
                    .arg(scope)
                    .arg("name")
                    .arg(name),
                value,
                "store secret in Linux Secret Service",
            ),
            SecretStoreBackend::WindowsCredentialManager => run_status(
                &self.runtime_root,
                HostCommand::new("cmdkey")
                    .arg(format!("/add:{}", secret_service_name(scope, name)?))
                    .arg(format!("/user:{scope}"))
                    .arg(format!("/pass:{value}")),
                "store secret in Windows Credential Manager",
            ),
            SecretStoreBackend::FileFallback => {
                self.put_file_secret(scope, name, value).map(|_| ())
            }
        }
    }

    fn get_native_secret(
        &self,
        backend: &SecretStoreBackend,
        scope: &str,
        name: &str,
    ) -> RefineResult<String> {
        match backend {
            SecretStoreBackend::MacosKeychain => run_output(
                &self.runtime_root,
                HostCommand::new("security")
                    .arg("find-generic-password")
                    .arg("-w")
                    .arg("-s")
                    .arg(secret_service_name(scope, name)?)
                    .arg("-a")
                    .arg(scope),
                "read secret from macOS Keychain",
            ),
            SecretStoreBackend::LinuxSecretService => run_output(
                &self.runtime_root,
                HostCommand::new("secret-tool")
                    .arg("lookup")
                    .arg("application")
                    .arg("refine")
                    .arg("scope")
                    .arg(scope)
                    .arg("name")
                    .arg(name),
                "read secret from Linux Secret Service",
            ),
            SecretStoreBackend::WindowsCredentialManager => Err(RefineError::NotImplemented(
                "Windows Credential Manager does not expose secret reads through cmdkey"
                    .to_string(),
            )),
            SecretStoreBackend::FileFallback => {
                self.get_file_secret(scope, name).map(|secret| secret.value)
            }
        }
    }

    fn delete_native_secret(
        &self,
        backend: &SecretStoreBackend,
        scope: &str,
        name: &str,
    ) -> RefineResult<()> {
        match backend {
            SecretStoreBackend::MacosKeychain => run_status(
                &self.runtime_root,
                HostCommand::new("security")
                    .arg("delete-generic-password")
                    .arg("-s")
                    .arg(secret_service_name(scope, name)?)
                    .arg("-a")
                    .arg(scope),
                "delete secret from macOS Keychain",
            ),
            SecretStoreBackend::LinuxSecretService => run_status(
                &self.runtime_root,
                HostCommand::new("secret-tool")
                    .arg("clear")
                    .arg("application")
                    .arg("refine")
                    .arg("scope")
                    .arg(scope)
                    .arg("name")
                    .arg(name),
                "delete secret from Linux Secret Service",
            ),
            SecretStoreBackend::WindowsCredentialManager => run_status(
                &self.runtime_root,
                HostCommand::new("cmdkey")
                    .arg(format!("/delete:{}", secret_service_name(scope, name)?)),
                "delete secret from Windows Credential Manager",
            ),
            SecretStoreBackend::FileFallback => self.delete_file_secret(scope, name),
        }
    }
}

impl SecretStore for NativeSecretStore {
    fn backend_status(&self) -> SecretStoreStatus {
        SecretStoreStatus {
            backend: self.preferred_backend.clone(),
            native: self.preferred_backend.native(),
        }
    }

    fn put_secret(&self, scope: &str, name: &str, value: &str) -> RefineResult<SecretMetadata> {
        validate_secret_id(scope)?;
        validate_secret_id(name)?;
        if value.is_empty() {
            return Err(RefineError::InvalidInput(
                "secret value is required".to_string(),
            ));
        }
        let backend = self.preferred_backend.clone();
        let updated_at = now_timestamp();
        if backend.native() && self.put_native_secret(&backend, scope, name, value).is_ok() {
            return self.upsert_index(SecretMetadata {
                scope: scope.to_string(),
                name: name.to_string(),
                backend: backend.clone(),
                native: true,
                updated_at,
            });
        }
        self.put_file_secret(scope, name, value)
    }

    fn get_secret(&self, scope: &str, name: &str) -> RefineResult<SecretValue> {
        validate_secret_id(scope)?;
        validate_secret_id(name)?;
        let metadata = self
            .load_index()?
            .into_iter()
            .find(|entry| entry.scope == scope && entry.name == name)
            .ok_or_else(|| RefineError::NotFound(format!("secret {scope}/{name} was not found")))?;
        let value = if metadata.backend.native() {
            self.get_native_secret(&metadata.backend, scope, name)
                .or_else(|_| self.get_file_secret(scope, name).map(|secret| secret.value))?
        } else {
            self.get_file_secret(scope, name)?.value
        };
        Ok(SecretValue { metadata, value })
    }

    fn delete_secret(&self, scope: &str, name: &str) -> RefineResult<SecretMetadata> {
        validate_secret_id(scope)?;
        validate_secret_id(name)?;
        let metadata = self.remove_from_index(scope, name)?;
        if metadata.backend.native() {
            let _ = self.delete_native_secret(&metadata.backend, scope, name);
        }
        let _ = self.delete_file_secret(scope, name);
        Ok(metadata)
    }

    fn list_secrets(&self) -> RefineResult<Vec<SecretMetadata>> {
        self.load_index()
    }
}

#[derive(Clone, Debug)]
pub struct FileSecurityService {
    pub runtime_root: PathBuf,
    pub allowed_commands: BTreeSet<String>,
}

impl FileSecurityService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            allowed_commands: BTreeSet::new(),
        }
    }

    pub fn with_allowed_commands(
        runtime_root: impl Into<PathBuf>,
        allowed_commands: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            allowed_commands: allowed_commands
                .into_iter()
                .map(|command| command.into())
                .collect(),
        }
    }

    pub fn from_project_settings(
        runtime_root: impl Into<PathBuf>,
        durable_root: impl Into<PathBuf>,
    ) -> RefineResult<Self> {
        let runtime_root = runtime_root.into();
        let durable_root = durable_root.into();
        let settings = FileSettingsService::new(durable_root).load()?;
        let allowed_commands = settings
            .get("allowed_commands")
            .and_then(|value| value.as_str())
            .map(parse_allowed_commands)
            .unwrap_or_default();
        Ok(Self::with_allowed_commands(runtime_root, allowed_commands))
    }

    pub fn audit_path(&self) -> PathBuf {
        self.runtime_root.join(SECURITY_AUDIT_FILE)
    }

    pub fn authorize_host_command(&self, actor: &str, command: &str) -> RefineResult<()> {
        let actor = actor.trim();
        let command = command.trim();
        if actor.is_empty() || command.is_empty() {
            return Err(RefineError::InvalidInput(
                "audit actor and command are required".to_string(),
            ));
        }
        if !self.command_allowed(command) {
            self.append_audit_event(actor, command, "denied")?;
            return Err(RefineError::Unauthorized(format!(
                "host command is not authorized: {command}"
            )));
        }
        self.append_audit_event(actor, command, "authorized")
    }

    fn command_allowed(&self, command: &str) -> bool {
        if self.allowed_commands.is_empty() {
            return true;
        }
        let command = command.trim();
        if self.allowed_commands.contains(command) {
            return true;
        }
        command
            .split_whitespace()
            .next()
            .is_some_and(|program| self.allowed_commands.contains(program))
    }

    fn append_audit_event(&self, actor: &str, command: &str, outcome: &str) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let line = serde_json::to_string(&json!({
            "actor": actor,
            "command": command,
            "outcome": outcome,
            "created_at": now_timestamp()
        }))
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode security audit event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.audit_path())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open security audit {}: {error}",
                    self.audit_path().display()
                ))
            })?;
        writeln!(file, "{line}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append security audit {}: {error}",
                self.audit_path().display()
            ))
        })
    }
}

impl SecurityService for FileSecurityService {
    fn redact(&self, value: &str) -> String {
        redact_assignment(value, "token")
    }

    fn audit(&self, actor: &str, command: &str) -> RefineResult<()> {
        let actor = actor.trim();
        let command = command.trim();
        if actor.is_empty() || command.is_empty() {
            return Err(RefineError::InvalidInput(
                "audit actor and command are required".to_string(),
            ));
        }
        self.append_audit_event(actor, command, "recorded")
    }
}

fn redact_assignment(value: &str, key: &str) -> String {
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

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn parse_allowed_commands(raw: &str) -> BTreeSet<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
        .collect()
}

fn detect_secret_backend() -> SecretStoreBackend {
    match std::env::consts::OS {
        "macos" if command_available("security") => SecretStoreBackend::MacosKeychain,
        "windows" if command_available("cmdkey") => SecretStoreBackend::WindowsCredentialManager,
        "linux" if command_available("secret-tool") => SecretStoreBackend::LinuxSecretService,
        _ => SecretStoreBackend::FileFallback,
    }
}

fn command_available(command: &str) -> bool {
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

fn validate_secret_id(value: &str) -> RefineResult<()> {
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

fn secret_storage_key(scope: &str, name: &str) -> RefineResult<String> {
    validate_secret_id(scope)?;
    validate_secret_id(name)?;
    Ok(format!("{scope}--{name}"))
}

fn secret_service_name(scope: &str, name: &str) -> RefineResult<String> {
    Ok(format!("refine.{}", secret_storage_key(scope, name)?))
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> RefineResult<()> {
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
struct HostCommand {
    program: String,
    args: Vec<String>,
}

impl HostCommand {
    fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    fn authorization_command(&self) -> String {
        self.program.clone()
    }
}

fn run_status(runtime_root: &Path, command: HostCommand, action: &str) -> RefineResult<()> {
    let output = run_managed_command(runtime_root, command, None, action)?;
    if output.success() {
        return Ok(());
    }
    Err(RefineError::Degraded(format!(
        "failed to {action}: {}",
        output.stderr.trim()
    )))
}

fn run_output(runtime_root: &Path, command: HostCommand, action: &str) -> RefineResult<String> {
    let output = run_managed_command(runtime_root, command, None, action)?;
    if output.success() {
        return Ok(output.stdout.trim().to_string());
    }
    Err(RefineError::Degraded(format!(
        "failed to {action}: {}",
        output.stderr.trim()
    )))
}

fn run_with_stdin(
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

fn run_managed_command(
    runtime_root: &Path,
    command: HostCommand,
    stdin: Option<String>,
    action: &str,
) -> RefineResult<crate::core::host::process_supervision::ManagedProcessOutput> {
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
        })
        .map_err(|error| RefineError::Io(format!("failed to {action}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_security_service_audits_and_enforces_host_command_allowlist() {
        let temp_root = unique_temp_dir("security");
        let security =
            FileSecurityService::with_allowed_commands(&temp_root, ["gap.create", "gap.edit"]);

        assert_eq!(
            security.redact("Authorization token=secret"),
            "Authorization token=[redacted]"
        );
        security.audit("cli", "gap.edit").unwrap();
        security
            .authorize_host_command("process_supervisor", "gap.create --dry-run")
            .unwrap();
        assert!(
            security
                .authorize_host_command("process_supervisor", "system.shell")
                .is_err()
        );
        let audit = fs::read_to_string(security.audit_path()).unwrap();
        assert!(audit.contains("authorized"));
        assert!(audit.contains("denied"));
        assert!(audit.contains("recorded"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_security_service_loads_allowed_commands_from_project_settings() {
        let temp_root = unique_temp_dir("security-settings");
        let runtime_root = temp_root.join("run");
        let durable_root = temp_root.join(".refine");
        FileSettingsService::new(&durable_root)
            .update(&serde_json::json!({
                "allowed_commands": "printf, npm run test\nplaywright"
            }))
            .unwrap();

        let security =
            FileSecurityService::from_project_settings(&runtime_root, &durable_root).unwrap();

        assert!(
            security
                .authorize_host_command("quality", "printf ok")
                .is_ok()
        );
        assert!(
            security
                .authorize_host_command("quality", "npm run test")
                .is_ok()
        );
        assert!(
            security
                .authorize_host_command("quality", "rm -rf target")
                .is_err()
        );

        fs::remove_dir_all(temp_root).unwrap_or(());
    }

    #[test]
    fn native_secret_store_persists_fallback_secrets_with_metadata() {
        let temp_root = unique_temp_dir("secret-store");
        let store = NativeSecretStore::with_backend(&temp_root, SecretStoreBackend::FileFallback);

        let metadata = store
            .put_secret("provider", "smoke_ai_token", "secret-value")
            .unwrap();
        assert_eq!(metadata.scope, "provider");
        assert_eq!(metadata.name, "smoke_ai_token");
        assert_eq!(metadata.backend, SecretStoreBackend::FileFallback);
        assert!(!metadata.native);

        let listed = store.list_secrets().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "smoke_ai_token");
        let secret = store.get_secret("provider", "smoke_ai_token").unwrap();
        assert_eq!(secret.value, "secret-value");

        let deleted = store.delete_secret("provider", "smoke_ai_token").unwrap();
        assert_eq!(deleted.name, "smoke_ai_token");
        assert!(store.list_secrets().unwrap().is_empty());
        assert!(store.get_secret("provider", "smoke_ai_token").is_err());

        fs::remove_dir_all(temp_root).unwrap_or(());
    }

    #[test]
    fn native_secret_store_rejects_invalid_secret_names() {
        let temp_root = unique_temp_dir("secret-store-invalid");
        let store = NativeSecretStore::with_backend(&temp_root, SecretStoreBackend::FileFallback);

        assert!(store.put_secret("provider", "bad/name", "value").is_err());
        assert!(store.put_secret("provider", "empty", "").is_err());
        assert_eq!(
            store.backend_status().backend,
            SecretStoreBackend::FileFallback
        );

        fs::remove_dir_all(temp_root).unwrap_or(());
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
