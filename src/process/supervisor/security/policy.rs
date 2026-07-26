use super::*;

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
        refine_dir: impl Into<PathBuf>,
    ) -> RefineResult<Self> {
        let runtime_root = runtime_root.into();
        let refine_dir = refine_dir.into();
        let settings = FileSettingsService::with_active_root(refine_dir, &runtime_root).load()?;
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
        if self.allowed_commands.is_empty() {
            return Ok(());
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
