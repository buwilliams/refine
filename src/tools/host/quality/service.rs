use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::log::LogEntry;
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::process::supervisor::security::FileSecurityService;

use super::types::*;

#[derive(Clone, Debug)]
pub struct FileQualityService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

impl FileQualityService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn load_settings(&self) -> RefineResult<QualitySettings> {
        let stored = self.read_stored_settings()?;
        Ok(QualitySettings {
            configured: !stored.business_requirements.trim().is_empty()
                && !stored.instructions.trim().is_empty(),
            business_requirements: stored.business_requirements,
            instructions: stored.instructions,
            enabled: stored.enabled,
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
                DEFAULT_INSTRUCTIONS.to_string()
            } else {
                trimmed.to_string()
            };
        }
        if let Some(enabled) = patch.enabled {
            stored.enabled = boolish_setting(&enabled);
        }
        if let Some(timing) = patch.timing {
            stored.timing = normalize_timing(&timing)?;
        }
        self.write_stored_settings(&stored)?;
        self.load_settings()
    }

    fn read_stored_settings(&self) -> RefineResult<StoredQualitySettings> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(StoredQualitySettings::default());
        }
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read quality settings {}: {error}",
                path.display()
            ))
        })?;
        let raw = serde_json::from_str::<StoredQualitySettings>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse quality settings {}: {error}",
                path.display()
            ))
        })?;
        Ok(raw.normalized())
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
        fs::write(&path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write quality settings {}: {error}",
                path.display()
            ))
        })
    }

    fn settings_path(&self) -> PathBuf {
        self.refine_dir.join(SETTINGS_FILE)
    }

    fn project_root(&self) -> PathBuf {
        self.refine_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.refine_dir.clone())
    }

    fn security(&self) -> RefineResult<FileSecurityService> {
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| self.refine_dir.join("runtime"));
        FileSecurityService::from_project_settings(runtime_root, &self.refine_dir)
    }

    fn run_quality_process(
        &self,
        command: String,
        args: Vec<String>,
        authorization_command: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<crate::process::subprocess::ManagedProcessOutput> {
        let security = self.security()?;
        security.authorize_host_command("quality", authorization_command)?;
        FileProcessSupervisor::with_allowed_commands(
            security.runtime_root,
            security.allowed_commands.iter().cloned(),
        )
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Quality,
            command,
            args,
            cwd: Some(self.project_root().display().to_string()),
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: Some(authorization_command.to_string()),
            sensitive: false,
            metadata: process_metadata,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredQualitySettings {
    business_requirements: String,
    instructions: String,
    enabled: String,
    timing: String,
}

impl StoredQualitySettings {
    fn normalized(self) -> Self {
        Self {
            business_requirements: self.business_requirements.trim().to_string(),
            instructions: if self.instructions.trim().is_empty() {
                DEFAULT_INSTRUCTIONS.to_string()
            } else {
                self.instructions.trim().to_string()
            },
            enabled: boolish_string(&self.enabled),
            timing: normalize_timing_lossy(&self.timing),
        }
    }
}

impl Default for StoredQualitySettings {
    fn default() -> Self {
        Self {
            business_requirements: String::new(),
            instructions: DEFAULT_INSTRUCTIONS.to_string(),
            enabled: "0".to_string(),
            timing: PRE_MERGE.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckRequest {
    pub owner_id: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub process_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckResult {
    pub owner_id: String,
    pub ok: bool,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityOperationResult {
    pub operation: OperationHandle,
    pub result: QualityCheckResult,
}

pub trait QualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult>;
    fn screenshots(&self, owner_id: &str) -> RefineResult<Vec<String>>;
    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult>;
    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult>;
}

#[derive(Clone, Debug)]
pub struct QualityOperationRunner {
    pub refine_dir: PathBuf,
    pub runtime_root: PathBuf,
}

impl QualityOperationRunner {
    pub fn new(refine_dir: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityOperationResult> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let operation = registry.register(&format!("quality:{}", request.owner_id))?;
        registry.append_log(
            &operation.id,
            quality_operation_log(
                &request.owner_id,
                "info",
                "Quality checks started",
                Some(json!({"command": request.command})),
            ),
        )?;
        let service = FileQualityService::with_runtime_root(&self.refine_dir, &self.runtime_root);
        let result = service.run_checks(request)?;
        registry.append_log(
            &operation.id,
            quality_operation_log(
                &result.owner_id,
                if result.ok { "info" } else { "error" },
                if result.ok {
                    "Quality checks passed"
                } else {
                    "Quality checks failed"
                },
                Some(json!({
                    "diagnostics": &result.diagnostics,
                    "ok": result.ok
                })),
            ),
        )?;
        let operation = registry.finish(
            &operation.id,
            if result.ok {
                OperationState::Succeeded
            } else {
                OperationState::Failed
            },
        )?;
        Ok(QualityOperationResult { operation, result })
    }
}

impl QualityService for FileQualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult> {
        if request.command.trim().is_empty() {
            return Ok(QualityCheckResult {
                owner_id: request.owner_id,
                ok: true,
                diagnostics: vec!["No quality command configured.".to_string()],
            });
        }
        let (shell, args) = shell_program_args(&request.command);
        let output =
            self.run_quality_process(shell, args, &request.command, request.process_metadata)?;
        let ok = output.success();
        let exit_code = output.process.exit_code;
        let stdout = output.stdout;
        let stderr = output.stderr;
        let mut diagnostics = Vec::new();
        if let Some(stdout) = tail_text(&stdout, 4000) {
            diagnostics.push(stdout);
        }
        if let Some(stderr) = tail_text(&stderr, 4000) {
            diagnostics.push(stderr);
        }
        if diagnostics.is_empty() {
            diagnostics.push(format!(
                "quality command exited {}",
                exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            ));
        }
        Ok(QualityCheckResult {
            owner_id: request.owner_id,
            ok,
            diagnostics,
        })
    }

    fn screenshots(&self, _owner_id: &str) -> RefineResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult> {
        let baseline_path = PathBuf::from(baseline);
        let candidate_path = PathBuf::from(candidate);
        let baseline_bytes = fs::read(&baseline_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read baseline artifact {}: {error}",
                baseline_path.display()
            ))
        })?;
        let candidate_bytes = fs::read(&candidate_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read candidate artifact {}: {error}",
                candidate_path.display()
            ))
        })?;
        let ok = baseline_bytes == candidate_bytes;
        Ok(QualityCheckResult {
            owner_id: format!("{baseline}:{candidate}"),
            ok,
            diagnostics: vec![if ok {
                "artifacts match exactly".to_string()
            } else {
                format!(
                    "artifacts differ: baseline {} bytes, candidate {} bytes",
                    baseline_bytes.len(),
                    candidate_bytes.len()
                )
            }],
        })
    }

    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult> {
        let settings = self.read_stored_settings()?;
        if settings.enabled != "1" {
            return Ok(QualityCheckResult {
                owner_id: owner_id.to_string(),
                ok: true,
                diagnostics: vec!["Quality gate is disabled.".to_string()],
            });
        }
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: true,
            diagnostics: vec![
                "Quality gate is enabled; workflow QA runs the target-app test command."
                    .to_string(),
            ],
        })
    }
}

pub(super) fn normalize_timing(value: &str) -> RefineResult<String> {
    match value.trim() {
        PRE_MERGE => Ok(PRE_MERGE.to_string()),
        POST_BUILD | "post_rebuild" => Ok(POST_BUILD.to_string()),
        _ => Err(RefineError::InvalidInput(
            "quality timing must be one of pre_merge, post_build".to_string(),
        )),
    }
}

pub(super) fn normalize_timing_lossy(value: &str) -> String {
    if value.trim() == POST_BUILD {
        POST_BUILD.to_string()
    } else {
        PRE_MERGE.to_string()
    }
}

pub(super) fn boolish_setting(value: &serde_json::Value) -> String {
    if value_is_truthy(value) {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

pub(super) fn boolish_string(value: &str) -> String {
    if matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    ) {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

pub(super) fn value_is_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_i64().unwrap_or_default() != 0,
        serde_json::Value::String(value) => {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        _ => false,
    }
}

pub(super) fn tail_text(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let count = trimmed.chars().count();
    if count <= max_chars {
        Some(trimmed.to_string())
    } else {
        Some(trimmed.chars().skip(count - max_chars).collect())
    }
}

pub(super) fn quality_operation_log(
    owner_id: &str,
    severity: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> LogEntry {
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "quality".to_string(),
        message: message.to_string(),
        details: details.and_then(|value| value.as_object().cloned()),
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        gap_id: owner_id
            .strip_prefix("GAP")
            .map(|_| owner_id.to_string())
            .or_else(|| owner_id.strip_prefix("gap:").map(ToString::to_string)),
    }
}

pub(super) fn shell_program_args(command: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
