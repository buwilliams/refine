use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use serde_json::json;

use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::sessions::{active_session_tokens, validate_session_token};

pub const SECURITY_AUDIT_FILE: &str = "security-audit.jsonl";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthToken {
    pub token: String,
}

pub trait SecurityService {
    fn authorize_mutation(&self, token: &AuthToken, command: &str) -> RefineResult<()>;
    fn redact(&self, value: &str) -> String;
    fn audit(&self, actor: &str, command: &str) -> RefineResult<()>;
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

    pub fn audit_path(&self) -> PathBuf {
        self.runtime_root.join(SECURITY_AUDIT_FILE)
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
    fn authorize_mutation(&self, token: &AuthToken, command: &str) -> RefineResult<()> {
        let command = command.trim();
        if command.is_empty() {
            return Err(RefineError::InvalidInput(
                "authorized command is required".to_string(),
            ));
        }
        validate_session_token(&self.runtime_root, &token.token)?;
        if !self.allowed_commands.is_empty() && !self.allowed_commands.contains(command) {
            self.append_audit_event("local_surface", command, "denied")?;
            return Err(RefineError::Unauthorized(format!(
                "command {command} is not authorized for this surface"
            )));
        }
        self.append_audit_event("local_surface", command, "authorized")
    }

    fn redact(&self, value: &str) -> String {
        let mut redacted = value.to_string();
        if let Ok(tokens) = active_session_tokens(&self.runtime_root) {
            for token in tokens {
                redacted = redacted.replace(&token, "[redacted]");
            }
        }
        redact_assignment(&redacted, "token")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::supervisor::sessions::{FileSessionService, SessionService, SurfaceKind};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_security_service_authorizes_local_session_tokens_and_audits() {
        let temp_root = unique_temp_dir("security");
        let sessions = FileSessionService::new(&temp_root, "http://127.0.0.1:8080");
        let session = sessions
            .authenticate_local_surface(SurfaceKind::Browser)
            .unwrap();
        let security =
            FileSecurityService::with_allowed_commands(&temp_root, ["gap.create", "gap.edit"]);

        security
            .authorize_mutation(
                &AuthToken {
                    token: session.token.clone(),
                },
                "gap.create",
            )
            .unwrap();
        assert!(
            security
                .authorize_mutation(
                    &AuthToken {
                        token: session.token.clone(),
                    },
                    "system.shell"
                )
                .is_err()
        );
        assert!(
            security
                .redact(&format!("Authorization token={}", session.token))
                .contains("[redacted]")
        );
        security.audit("cli", "gap.edit").unwrap();
        let audit = fs::read_to_string(security.audit_path()).unwrap();
        assert!(audit.contains("authorized"));
        assert!(audit.contains("denied"));
        assert!(audit.contains("recorded"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_security_service_rejects_unknown_tokens() {
        let temp_root = unique_temp_dir("security-unknown");
        let security = FileSecurityService::new(&temp_root);

        assert!(
            security
                .authorize_mutation(
                    &AuthToken {
                        token: "missing".to_string(),
                    },
                    "gap.create"
                )
                .is_err()
        );
        assert!(security.audit("", "gap.create").is_err());

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
