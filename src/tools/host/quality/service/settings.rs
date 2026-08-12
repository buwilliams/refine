use super::*;

impl FileQualityService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: None,
            #[cfg(test)]
            migration_failure_after_stage: false,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: Some(runtime_root.into()),
            #[cfg(test)]
            migration_failure_after_stage: false,
        }
    }

    pub fn load_settings(&self) -> RefineResult<QualitySettings> {
        let stored = self.read_stored_settings()?;
        Ok(QualitySettings {
            configured: !stored.tests.is_empty() || !stored.legacy_commands.is_empty(),
            business_requirements: stored.business_requirements,
            instructions: stored.instructions,
            tests: stored.tests,
            legacy_commands: stored.legacy_commands,
            enabled: "1".to_string(),
            timing: stored.timing,
        })
    }

    pub fn save_settings(&self, patch: QualitySettingsPatch) -> RefineResult<QualitySettings> {
        let mut stored = self.read_stored_settings()?;
        if let Some(requirements) = patch.business_requirements {
            stored.business_requirements = requirements.trim().to_string();
        }
        if let Some(instructions) = patch.instructions {
            let trimmed = instructions.trim();
            stored.instructions = if trimmed.is_empty() {
                default_quality_instructions().to_string()
            } else {
                trimmed.to_string()
            };
        }
        if let Some(tests) = patch.tests {
            let tests = normalize_tests(tests);
            // Replacing migrated command checks requires a non-empty explicit test set. Clearing
            // the editor cannot silently retire checks that were enforced before upgrade.
            if !tests.is_empty() {
                stored.legacy_commands.clear();
            }
            stored.tests = tests;
        }
        // `enabled` remains accepted on the compatibility wire shape, but every candidate is
        // evaluated. It cannot disable Quality.
        if let Some(timing) = patch.timing {
            stored.timing = normalize_timing(&timing)?;
        }
        stored.migration_version = SETTINGS_MIGRATION_VERSION;
        self.write_stored_settings(&stored)?;
        self.load_settings()
    }

    fn read_stored_settings(&self) -> RefineResult<StoredQualitySettings> {
        let path = self.settings_path();
        let existed = path.exists();
        let mut stored = if existed {
            let bytes = fs::read_to_string(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read quality settings {}: {error}",
                    path.display()
                ))
            })?;
            serde_json::from_str::<StoredQualitySettings>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse quality settings {}: {error}",
                    path.display()
                ))
            })?
        } else {
            StoredQualitySettings::default()
        };
        if stored.migration_version < SETTINGS_MIGRATION_VERSION {
            let node_service = FileNodeRegistryService::new(&self.refine_dir);
            let mut registry = node_service.load_registry()?;
            if !existed {
                let timings = registry
                    .nodes
                    .iter()
                    .map(|node| {
                        node.settings
                            .get("quality_timing")
                            .and_then(Value::as_str)
                            .map(normalize_timing_lossy)
                            .unwrap_or_else(|| PRE_MERGE.to_string())
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                if timings.len() > 1 {
                    return Err(RefineError::Conflict(
                        "legacy Node quality_timing values diverge; migration cannot choose one project-wide Quality timing"
                            .to_string(),
                    ));
                }
                stored.timing = timings
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| PRE_MERGE.to_string());
            }

            let mut commands = stored.legacy_commands.clone();
            for node in &registry.nodes {
                if legacy_quality_enabled(&node.settings) {
                    commands.extend(enabled_legacy_commands(&node.settings));
                }
            }
            stored.legacy_commands = normalize_commands(commands);

            // Stage imported state without advancing the migration marker. If Node cleanup or
            // the final write fails, retry sees both the staged commands and remaining legacy
            // state, so enforced QA cannot disappear between attempts.
            self.write_stored_settings(&stored)?;
            #[cfg(test)]
            if self.migration_failure_after_stage {
                return Err(RefineError::Io(
                    "injected Quality migration failure after staged settings write".to_string(),
                ));
            }
            let mut registry_changed = false;
            for node in &mut registry.nodes {
                registry_changed |= node.settings.remove("quality_timing").is_some();
            }
            if registry_changed {
                node_service.save_registry(&registry)?;
            }
            stored.migration_version = SETTINGS_MIGRATION_VERSION;
            self.write_stored_settings(&stored)?;
        }
        Ok(stored.normalized())
    }

    fn write_stored_settings(&self, settings: &StoredQualitySettings) -> RefineResult<()> {
        let path = self.settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create quality settings directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let encoded =
            serde_json::to_string_pretty(&settings.clone().normalized()).map_err(|error| {
                RefineError::Serialization(format!("failed to encode quality settings: {error}"))
            })?;
        write_json_atomically(&path, format!("{encoded}\n").as_bytes(), "quality settings")
    }

    fn settings_path(&self) -> PathBuf {
        self.refine_dir.join(SETTINGS_FILE)
    }

    pub(super) fn run_observed_command(
        &self,
        command: &str,
        cwd: &Path,
        metadata: Map<String, Value>,
    ) -> RefineResult<ObservedExecution> {
        let runtime_root = self.runtime_root.clone().ok_or_else(|| {
            RefineError::Degraded("runtime root is required for Quality".to_string())
        })?;
        let security = FileSecurityService::from_project_settings(&runtime_root, &self.refine_dir)?;
        security.authorize_host_command("quality", command)?;
        let (shell, args) = shell_program_args(command);
        let observed_shell = shell.clone();
        let output = FileProcessSupervisor::with_allowed_commands(
            runtime_root,
            security.allowed_commands.iter().cloned(),
        )
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Quality,
            command: shell,
            args,
            cwd: Some(cwd.display().to_string()),
            env: Vec::new(),
            stdin: None,
            limits: Some(ProcessResourceLimits {
                kill_on_parent_exit: true,
                ..Default::default()
            }),
            authorization_command: Some(command.to_string()),
            sensitive: false,
            metadata,
        })?;
        Ok(ObservedExecution {
            process_id: output.process.id,
            shell: observed_shell,
            exit_code: output.process.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    pub(super) fn ensure_operation_active(
        &self,
        request: &QualityCheckRequest,
        boundary: &str,
    ) -> RefineResult<()> {
        let operation_id = request
            .process_metadata
            .get("operation_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RefineError::Degraded(
                    "Quality supervised work is missing its operation_id".to_string(),
                )
            })?;
        let runtime_root = self.runtime_root.as_ref().ok_or_else(|| {
            RefineError::Degraded("runtime root is required for Quality".to_string())
        })?;
        let operation = FileOperationRegistry::new(runtime_root).status(operation_id)?;
        if matches!(
            operation.state,
            OperationState::Pending | OperationState::Running
        ) {
            return Ok(());
        }
        Err(RefineError::Conflict(format!(
            "Quality operation {operation_id} is {}; cancellation prevented {boundary}",
            operation.state.as_api_status()
        )))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
struct StoredQualitySettings {
    business_requirements: String,
    instructions: String,
    tests: Vec<String>,
    legacy_commands: Vec<String>,
    timing: String,
    migration_version: u32,
}

impl StoredQualitySettings {
    fn normalized(self) -> Self {
        Self {
            business_requirements: self.business_requirements.trim().to_string(),
            instructions: if self.instructions.trim().is_empty() {
                default_quality_instructions().to_string()
            } else {
                self.instructions.trim().to_string()
            },
            tests: normalize_tests(self.tests),
            legacy_commands: normalize_commands(self.legacy_commands),
            timing: normalize_timing_lossy(&self.timing),
            migration_version: self.migration_version,
        }
    }
}

impl Default for StoredQualitySettings {
    fn default() -> Self {
        Self {
            business_requirements: String::new(),
            instructions: default_quality_instructions().to_string(),
            tests: Vec::new(),
            legacy_commands: Vec::new(),
            timing: PRE_MERGE.to_string(),
            migration_version: 0,
        }
    }
}
