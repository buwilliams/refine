use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};

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

mod host_commands;
mod policy;
mod secrets;

pub use policy::FileSecurityService;
pub use secrets::NativeSecretStore;

use host_commands::*;

#[cfg(test)]
mod tests;
