use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::log::LogEntry;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::process::supervisor::security::FileSecurityService;
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::work_items::FileWorkItemService;

use super::types::*;

pub(super) const SETTINGS_MIGRATION_VERSION: u32 = 2;

#[derive(Clone, Debug)]
pub struct FileQualityService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
    #[cfg(test)]
    pub migration_failure_after_stage: bool,
}

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
                DEFAULT_INSTRUCTIONS.to_string()
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

    fn run_observed_command(
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
            exit_code: output.process.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn ensure_operation_active(
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
                DEFAULT_INSTRUCTIONS.to_string()
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
            instructions: DEFAULT_INSTRUCTIONS.to_string(),
            tests: Vec::new(),
            legacy_commands: Vec::new(),
            timing: PRE_MERGE.to_string(),
            migration_version: 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckRequest {
    pub owner_id: String,
    pub round_idx: usize,
    pub provider: String,
    pub cwd: String,
    pub candidate_commit: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub process_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityTestResult {
    pub test: String,
    pub status: String,
    pub evidence: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckResult {
    pub owner_id: String,
    pub ok: bool,
    pub summary: String,
    pub results: Vec<QualityTestResult>,
    pub diagnostics: Vec<String>,
    pub candidate_commit: String,
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
    pub target_root: PathBuf,
}

impl QualityOperationRunner {
    pub fn new(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
            target_root: target_root.into(),
        }
    }

    pub fn run_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<QualityOperationResult> {
        let (operation, request) =
            self.register_goal_checks(goal_id, provider, process_metadata)?;
        self.run_registered(&operation.id, request)
    }

    pub fn start_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<OperationHandle> {
        let (operation, request) =
            self.register_goal_checks(goal_id, provider, process_metadata)?;
        let runner = self.clone();
        let operation_id = operation.id.clone();
        thread::spawn(move || {
            let _ = runner.run_registered(&operation_id, request);
        });
        Ok(operation)
    }

    pub(super) fn register_goal_checks(
        &self,
        goal_id: &str,
        provider: &str,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<(OperationHandle, QualityCheckRequest)> {
        let goal_id = goal_id.trim();
        if goal_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "goal_id is required for Quality evaluation".to_string(),
            ));
        }
        let work_items = FileWorkItemService::new(&self.refine_dir);
        let summary = work_items.show_goal_summary(goal_id)?;
        let detail = work_items.show_goal_detail(goal_id)?;
        let candidate_commit = detail
            .get("candidate_commit")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no recorded candidate commit for Quality evaluation"
                ))
            })?
            .to_string();
        let branch = summary.goal.branch_name.as_deref().ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no candidate branch for Quality evaluation"
            ))
        })?;
        let cwd = FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root)
            .existing_worktree_for_branch(branch)?
            .ok_or_else(|| {
                RefineError::Conflict(format!("Goal {goal_id} candidate worktree was not found"))
            })?;
        let round_idx = summary.goal.round_count.checked_sub(1).ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no round to record Quality evidence"
            ))
        })?;
        let request = QualityCheckRequest {
            owner_id: goal_id.to_string(),
            round_idx,
            provider: provider.to_string(),
            cwd: cwd.display().to_string(),
            candidate_commit,
            process_metadata,
        };
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let owner = format!("quality:{goal_id}:{}", request.candidate_commit);
        let operation = registry.register_exclusive_with_request(
            &owner,
            json!({
                "goal_id": goal_id,
                "round_idx": round_idx,
                "provider": provider,
                "cwd": request.cwd,
                "candidate_commit": request.candidate_commit,
                "defer_cancellation_terminal": true
            }),
        )?;
        registry.append_log(
            &operation.id,
            quality_operation_log(
                goal_id,
                "info",
                "Quality checks started",
                Some(json!({
                    "provider": provider,
                    "cwd": request.cwd,
                    "candidate_commit": request.candidate_commit
                })),
            ),
        )?;
        Ok((operation, request))
    }

    pub(super) fn run_registered(
        &self,
        operation_id: &str,
        mut request: QualityCheckRequest,
    ) -> RefineResult<QualityOperationResult> {
        request
            .process_metadata
            .insert("operation_id".to_string(), json!(operation_id));
        request
            .process_metadata
            .insert("kind".to_string(), json!("quality"));
        request
            .process_metadata
            .insert("goal_id".to_string(), json!(&request.owner_id));
        request
            .process_metadata
            .insert("round_idx".to_string(), json!(request.round_idx));
        request.process_metadata.insert(
            "candidate_commit".to_string(),
            json!(&request.candidate_commit),
        );
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let service = FileQualityService::with_runtime_root(&self.refine_dir, &self.runtime_root);
        match service.run_checks(request.clone()) {
            Ok(result) => {
                registry.append_log(
                    operation_id,
                    quality_operation_log(
                        &request.owner_id,
                        if result.ok { "info" } else { "error" },
                        if result.ok {
                            "Quality checks passed"
                        } else {
                            "Quality checks failed"
                        },
                        Some(json!({
                            "summary": &result.summary,
                            "candidate_commit": &result.candidate_commit,
                            "results": &result.results,
                            "diagnostics": &result.diagnostics
                        })),
                    ),
                )?;
                let current = registry.status(operation_id)?;
                match current.state {
                    OperationState::Cancelling if cancellation_requested(&current) => {
                        let operation = self.settle_cancelled(&request, operation_id)?;
                        return Ok(QualityOperationResult { operation, result });
                    }
                    OperationState::Cancelled => {
                        self.record_cancelled(&request, operation_id)?;
                        return Ok(QualityOperationResult {
                            operation: current,
                            result,
                        });
                    }
                    OperationState::Cancelling | OperationState::Interrupted => {
                        return Ok(QualityOperationResult {
                            operation: current,
                            result,
                        });
                    }
                    _ => {}
                }
                if let Err(error) = self.record_result(&request, &result, operation_id) {
                    self.record_persistence_failure(operation_id, &request, &error);
                    return Err(error);
                }
                let operation = registry.finish_with_result(
                    operation_id,
                    if result.ok {
                        OperationState::Succeeded
                    } else {
                        OperationState::Failed
                    },
                    serde_json::to_value(&result).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to encode Quality operation result: {error}"
                        ))
                    })?,
                )?;
                if matches!(operation.state, OperationState::Cancelling)
                    && cancellation_requested(&operation)
                {
                    let operation = self.settle_cancelled(&request, operation_id)?;
                    return Ok(QualityOperationResult { operation, result });
                }
                if matches!(operation.state, OperationState::Cancelled) {
                    self.record_cancelled(&request, operation_id)?;
                }
                Ok(QualityOperationResult { operation, result })
            }
            Err(error) => {
                registry.append_log(
                    operation_id,
                    quality_operation_log(
                        &request.owner_id,
                        "error",
                        "Quality checks failed",
                        Some(json!({"error": error.to_string()})),
                    ),
                )?;
                let current = registry.status(operation_id)?;
                match current.state {
                    OperationState::Cancelling if cancellation_requested(&current) => {
                        self.settle_cancelled(&request, operation_id)?;
                        return Err(error);
                    }
                    OperationState::Cancelled => {
                        self.record_cancelled(&request, operation_id)?;
                        return Err(error);
                    }
                    OperationState::Cancelling | OperationState::Interrupted => {
                        return Err(error);
                    }
                    _ => {}
                }
                if let Err(persistence_error) = self.record_error(&request, &error, operation_id) {
                    self.record_persistence_failure(operation_id, &request, &persistence_error);
                    // Preserve provider and authentication failures verbatim while leaving the
                    // operation nonterminal for restart recovery.
                    return Err(error);
                }
                registry.fail_with_error(
                    operation_id,
                    json!({
                        "code": "quality_evaluation_failed",
                        "message": error.to_string()
                    }),
                )?;
                Err(error)
            }
        }
    }

    fn record_result(
        &self,
        request: &QualityCheckRequest,
        result: &QualityCheckResult,
        operation_id: &str,
    ) -> RefineResult<()> {
        let details = json!({
            "operation_id": operation_id,
            "candidate_commit": request.candidate_commit,
            "results": result.results,
            "diagnostics": result.diagnostics
        });
        FileWorkItemService::new(&self.refine_dir).update_goal_round_evaluation_summary(
            &request.owner_id,
            request.round_idx,
            &json!({
                "quality_state": if result.ok { "passed" } else { "failed" },
                "quality_message": result.summary,
                "quality_details": details,
                "quality_checked_at": now_timestamp()
            }),
        )?;
        self.append_goal_log(
            request,
            if result.ok { "info" } else { "error" },
            &result.summary,
            details,
        )
    }

    fn record_error(
        &self,
        request: &QualityCheckRequest,
        error: &RefineError,
        operation_id: &str,
    ) -> RefineResult<()> {
        let message = error.to_string();
        let details = json!({
            "operation_id": operation_id,
            "candidate_commit": request.candidate_commit,
            "error": message
        });
        FileWorkItemService::new(&self.refine_dir).update_goal_round_evaluation_summary(
            &request.owner_id,
            request.round_idx,
            &json!({
                "quality_state": "failed",
                "quality_message": message,
                "quality_details": details,
                "quality_checked_at": now_timestamp()
            }),
        )?;
        self.append_goal_log(request, "error", &message, details)
    }

    fn record_cancelled(
        &self,
        request: &QualityCheckRequest,
        operation_id: &str,
    ) -> RefineResult<()> {
        let message = "Quality checks cancelled.";
        let details = json!({
            "operation_id": operation_id,
            "candidate_commit": request.candidate_commit
        });
        let work_items = FileWorkItemService::new(&self.refine_dir);
        let detail = work_items.show_goal_detail(&request.owner_id)?;
        let summary_persisted = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(request.round_idx))
            .is_some_and(|round| {
                round.get("quality_state").and_then(Value::as_str) == Some("cancelled")
                    && round
                        .get("quality_details")
                        .and_then(|details| details.get("operation_id"))
                        .and_then(Value::as_str)
                        == Some(operation_id)
            });
        if !summary_persisted {
            work_items.update_goal_round_evaluation_summary(
                &request.owner_id,
                request.round_idx,
                &json!({
                    "quality_state": "cancelled",
                    "quality_message": message,
                    "quality_details": details,
                    "quality_checked_at": now_timestamp()
                }),
            )?;
        }
        let logs = FileLogService::new(&self.refine_dir).all_round_logs(&request.owner_id)?;
        let log_persisted = logs.iter().any(|entry| {
            entry.round_idx == Some(request.round_idx)
                && entry.entry.category == "quality"
                && entry.entry.message == message
                && entry
                    .entry
                    .details
                    .as_ref()
                    .and_then(|details| details.get("operation_id"))
                    .and_then(Value::as_str)
                    == Some(operation_id)
        });
        if !log_persisted {
            self.append_goal_log(request, "warning", message, details)?;
        }
        Ok(())
    }

    fn settle_cancelled(
        &self,
        request: &QualityCheckRequest,
        operation_id: &str,
    ) -> RefineResult<OperationHandle> {
        if let Err(error) = self.record_cancelled(request, operation_id) {
            self.record_persistence_failure(operation_id, request, &error);
            return Err(error);
        }
        FileOperationRegistry::new(&self.runtime_root).settle_cancellation(operation_id)
    }

    /// Replays incomplete Quality cancellation settlement after generic process recovery has
    /// confirmed that no owned provider or command remains alive.
    pub fn recover_cancelled_operations(&self) -> RefineResult<Vec<OperationHandle>> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let mut recovered = Vec::new();
        for operation in registry.recover()? {
            if !matches!(operation.state, OperationState::Cancelling)
                || operation
                    .request
                    .get("defer_cancellation_terminal")
                    .and_then(Value::as_bool)
                    != Some(true)
                || !cancellation_requested(&operation)
                || !operation.owner.starts_with("quality:")
            {
                continue;
            }
            let request = QualityCheckRequest {
                owner_id: required_operation_request_string(&operation, "goal_id")?,
                round_idx: operation
                    .request
                    .get("round_idx")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| {
                        RefineError::Serialization(format!(
                            "Quality operation {} has no valid round_idx for cancellation recovery",
                            operation.id
                        ))
                    })?,
                provider: required_operation_request_string(&operation, "provider")?,
                cwd: required_operation_request_string(&operation, "cwd")?,
                candidate_commit: required_operation_request_string(
                    &operation,
                    "candidate_commit",
                )?,
                process_metadata: Map::new(),
            };
            recovered.push(self.settle_cancelled(&request, &operation.id)?);
        }
        Ok(recovered)
    }

    fn append_goal_log(
        &self,
        request: &QualityCheckRequest,
        severity: &str,
        message: &str,
        details: Value,
    ) -> RefineResult<()> {
        FileLogService::new(&self.refine_dir).append_round_log(
            &request.owner_id,
            request.round_idx,
            LogEntry {
                datetime: now_timestamp(),
                severity: severity.to_string(),
                category: "quality".to_string(),
                message: message.to_string(),
                details: details.as_object().cloned(),
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                goal_id: Some(request.owner_id.clone()),
            },
        )?;
        Ok(())
    }

    fn record_persistence_failure(
        &self,
        operation_id: &str,
        request: &QualityCheckRequest,
        error: &RefineError,
    ) {
        let _ = FileOperationRegistry::new(&self.runtime_root).append_log(
            operation_id,
            quality_operation_log(
                &request.owner_id,
                "error",
                "Quality evidence persistence failed; operation remains nonterminal for recovery",
                Some(json!({"error": error.to_string()})),
            ),
        );
    }
}

fn required_operation_request_string(
    operation: &OperationHandle,
    field: &str,
) -> RefineResult<String> {
    operation
        .request
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            RefineError::Serialization(format!(
                "Quality operation {} has no valid {field} for cancellation recovery",
                operation.id
            ))
        })
}

fn cancellation_requested(operation: &OperationHandle) -> bool {
    operation
        .request
        .get("cancellation_requested")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

impl QualityService for FileQualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult> {
        let candidate_root = PathBuf::from(&request.cwd);
        verify_candidate(&candidate_root, &request.candidate_commit, "before")?;
        let settings = self.load_settings()?;
        let definitions = quality_test_definitions(&settings);
        if definitions.is_empty() {
            verify_candidate(&candidate_root, &request.candidate_commit, "after")?;
            return Ok(QualityCheckResult {
                owner_id: request.owner_id,
                ok: true,
                summary: "No Quality tests configured.".to_string(),
                results: Vec::new(),
                diagnostics: vec![
                    "No Quality tests or migrated commands are configured; evaluation was a no-op."
                        .to_string(),
                ],
                candidate_commit: request.candidate_commit,
            });
        }
        let test_names = definitions
            .iter()
            .map(|definition| definition.test.clone())
            .collect::<Vec<_>>();
        let tests_json = serde_json::to_string_pretty(&test_names).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Quality tests: {error}"))
        })?;
        let prompt = render(
            PromptTemplate::PostImplementationQuality,
            &[
                ("owner_id", &request.owner_id),
                ("candidate_cwd", &request.cwd),
                ("business_requirements", &settings.business_requirements),
                ("instructions", &settings.instructions),
                ("tests_json", &tests_json),
            ],
        );
        let runtime_root = self.runtime_root.clone().ok_or_else(|| {
            RefineError::Degraded("runtime root is required for Quality".to_string())
        })?;
        self.ensure_operation_active(&request, "provider launch")?;
        let output = HostAgentProviderService::with_runtime_root(&runtime_root).invoke(
            ProviderInvocation {
                provider: request.provider.clone(),
                prompt,
                session_id: None,
                cwd: Some(request.cwd.clone()),
                process_metadata: request.process_metadata.clone(),
            },
        )?;
        let plan = parse_quality_provider_output(&request.owner_id, &test_names, &output)?;
        let mut results = Vec::with_capacity(definitions.len());
        let mut diagnostics = plan.diagnostics;
        for (definition, planned) in definitions.iter().zip(plan.results) {
            let mut result = planned;
            if let Some(required) = definition.required_command.as_deref()
                && result.command != required
            {
                result.status = "failed".to_string();
                result.evidence = format!(
                    "Migrated Quality command must remain {required:?}; the agent proposed {:?}.",
                    result.command
                );
                diagnostics.push(result.evidence.clone());
                results.push(result);
                continue;
            }
            if result.command.trim().is_empty() {
                result.status = "failed".to_string();
                result.evidence =
                    "Pass claim rejected because no supervised command execution was requested."
                        .to_string();
                diagnostics.push(result.evidence.clone());
                results.push(result);
                continue;
            }
            let mut metadata = request.process_metadata.clone();
            metadata.insert("quality_test".to_string(), json!(&result.test));
            metadata.insert("quality_command".to_string(), json!(&result.command));
            self.ensure_operation_active(&request, "the next test command")?;
            let observed = self.run_observed_command(&result.command, &candidate_root, metadata)?;
            let observed_ok = observed.exit_code == Some(0);
            result.process_id = Some(observed.process_id.clone());
            result.exit_code = observed.exit_code;
            let observed_evidence = observed.evidence();
            if result.status != "passed" || !observed_ok || result.evidence.trim().is_empty() {
                result.status = "failed".to_string();
            }
            result.evidence = format!("{} Agent report: {}", observed_evidence, result.evidence);
            diagnostics.push(observed_evidence);
            results.push(result);
        }
        verify_candidate(&candidate_root, &request.candidate_commit, "after")?;
        let ok = results.iter().all(|result| result.status == "passed");
        Ok(QualityCheckResult {
            owner_id: request.owner_id,
            ok,
            summary: if ok {
                "All Quality tests passed with observed supervised evidence.".to_string()
            } else {
                "One or more Quality tests failed or lacked observed supervised evidence."
                    .to_string()
            },
            results,
            diagnostics,
            candidate_commit: request.candidate_commit,
        })
    }

    fn screenshots(&self, _owner_id: &str) -> RefineResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult> {
        let baseline_bytes = fs::read(baseline)
            .map_err(|error| RefineError::Io(format!("failed to read {baseline}: {error}")))?;
        let candidate_bytes = fs::read(candidate)
            .map_err(|error| RefineError::Io(format!("failed to read {candidate}: {error}")))?;
        let ok = baseline_bytes == candidate_bytes;
        Ok(QualityCheckResult {
            owner_id: format!("{baseline}:{candidate}"),
            ok,
            summary: if ok {
                "Artifacts match exactly.".to_string()
            } else {
                "Artifacts differ.".to_string()
            },
            results: Vec::new(),
            diagnostics: vec![if ok {
                "artifacts match exactly".to_string()
            } else {
                "artifacts differ".to_string()
            }],
            candidate_commit: String::new(),
        })
    }

    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult> {
        let settings = self.load_settings()?;
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: true,
            summary: "Quality evaluates every Goal candidate.".to_string(),
            results: Vec::new(),
            diagnostics: vec![format!(
                "Quality is active with {} plain-text test(s) and {} migrated command(s).",
                settings.tests.len(),
                settings.legacy_commands.len()
            )],
            candidate_commit: String::new(),
        })
    }
}

#[derive(Clone, Debug)]
struct QualityTestDefinition {
    test: String,
    required_command: Option<String>,
}

#[derive(Clone, Debug)]
struct ObservedExecution {
    process_id: String,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl ObservedExecution {
    fn evidence(&self) -> String {
        let mut evidence = format!(
            "Observed supervised process {} exited {}.",
            self.process_id,
            self.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "without an exit code".to_string())
        );
        if let Some(stdout) = tail_text(&self.stdout, 2000) {
            evidence.push_str(&format!(" stdout: {stdout}"));
        }
        if let Some(stderr) = tail_text(&self.stderr, 2000) {
            evidence.push_str(&format!(" stderr: {stderr}"));
        }
        evidence
    }
}

pub(crate) fn parse_quality_provider_output(
    owner_id: &str,
    configured_tests: &[String],
    output: &str,
) -> RefineResult<QualityCheckResult> {
    let value = parse_json_value(output).ok_or_else(|| {
        RefineError::Serialization(
            "Quality agent did not return the required JSON evaluation".to_string(),
        )
    })?;
    let returned = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RefineError::Serialization(
                "Quality agent response is missing a results array".to_string(),
            )
        })?;
    let mut results = Vec::with_capacity(configured_tests.len());
    let mut diagnostics = Vec::new();
    for test in configured_tests {
        let matches = returned
            .iter()
            .filter(|item| item.get("test").and_then(Value::as_str) == Some(test.as_str()))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            let evidence = if matches.is_empty() {
                "Quality agent omitted this configured test.".to_string()
            } else {
                "Quality agent returned this configured test more than once.".to_string()
            };
            diagnostics.push(evidence.clone());
            results.push(QualityTestResult {
                test: test.clone(),
                status: "failed".to_string(),
                evidence,
                command: String::new(),
                process_id: None,
                exit_code: None,
            });
            continue;
        }
        let item = matches[0];
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let evidence = item
            .get("evidence")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let valid_status = matches!(status.as_str(), "passed" | "failed");
        if !valid_status {
            diagnostics.push(format!(
                "Quality agent returned invalid status {status:?} for {test:?}."
            ));
        }
        if evidence.is_empty() {
            diagnostics.push(format!("Quality agent returned no evidence for {test:?}."));
        }
        results.push(QualityTestResult {
            test: test.clone(),
            status: if valid_status && !evidence.is_empty() {
                status
            } else {
                "failed".to_string()
            },
            evidence,
            command,
            process_id: None,
            exit_code: None,
        });
    }
    if returned.len() != configured_tests.len() {
        let mismatch = format!(
            "Quality agent returned {} result(s) for {} configured test(s).",
            returned.len(),
            configured_tests.len()
        );
        diagnostics.push(mismatch.clone());
        if let Some(result) = results.first_mut() {
            result.status = "failed".to_string();
            result.evidence = if result.evidence.is_empty() {
                mismatch
            } else {
                format!("{} {mismatch}", result.evidence)
            };
        }
    }
    diagnostics.push(output.trim().to_string());
    Ok(QualityCheckResult {
        owner_id: owner_id.to_string(),
        ok: false,
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        results,
        diagnostics,
        candidate_commit: String::new(),
    })
}

fn verify_candidate(root: &Path, expected_commit: &str, phase: &str) -> RefineResult<()> {
    let git = FileGitWorktreeService::new(root);
    let head = git.head_ref()?;
    let actual = head.commit.as_deref().unwrap_or("<unborn>");
    if actual != expected_commit {
        return Err(RefineError::Conflict(format!(
            "Quality {phase} check found candidate HEAD {actual}, expected recorded candidate {expected_commit}; user work was preserved"
        )));
    }
    let status = git.inspect(root.to_str().unwrap_or(""))?;
    if status.dirty_user_changes || !status.refine_owned_artifacts.is_empty() {
        return Err(RefineError::Conflict(format!(
            "Quality {phase} check found a dirty candidate index or worktree at {}; user work was preserved",
            root.display()
        )));
    }
    Ok(())
}

fn quality_test_definitions(settings: &QualitySettings) -> Vec<QualityTestDefinition> {
    let mut definitions = settings
        .tests
        .iter()
        .map(|test| QualityTestDefinition {
            test: test.clone(),
            required_command: None,
        })
        .collect::<Vec<_>>();
    definitions.extend(
        settings
            .legacy_commands
            .iter()
            .map(|command| QualityTestDefinition {
                test: format!("Migrated Quality command passes: {command}"),
                required_command: Some(command.clone()),
            }),
    );
    definitions
}

fn enabled_legacy_commands(settings: &Map<String, Value>) -> Vec<String> {
    let raw = settings
        .get("target_app_test_commands")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut commands = serde_json::from_str::<Value>(raw.trim())
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("enabled").and_then(Value::as_bool).unwrap_or(true))
        .filter_map(|item| {
            item.get("command")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    if commands.is_empty()
        && let Some(command) = settings
            .get("target_app_test_command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
    {
        commands.push(command.to_string());
    }
    normalize_commands(commands)
}

fn legacy_quality_enabled(settings: &Map<String, Value>) -> bool {
    match settings.get("quality_enabled") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or_default() != 0,
        Some(Value::String(value)) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => false,
    }
}

fn parse_json_value(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw.trim()).ok().or_else(|| {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        serde_json::from_str::<Value>(&raw[start..=end]).ok()
    })
}

fn normalize_tests(tests: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for test in tests {
        let test = test
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(1000)
            .collect::<String>();
        if !test.is_empty() && !normalized.contains(&test) {
            normalized.push(test);
        }
    }
    normalized
}

fn normalize_commands(commands: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for command in commands {
        let command = command.trim().chars().take(4000).collect::<String>();
        if !command.is_empty() && !normalized.contains(&command) {
            normalized.push(command);
        }
    }
    normalized
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
    if matches!(value.trim(), POST_BUILD | "post_rebuild") {
        POST_BUILD.to_string()
    } else {
        PRE_MERGE.to_string()
    }
}

fn shell_program_args(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    } else {
        (
            "sh".to_string(),
            vec!["-lc".to_string(), command.to_string()],
        )
    }
}

fn tail_text(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let chars = trimmed.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(max_chars);
    Some(chars[start..].iter().collect())
}

pub(super) fn quality_operation_log(
    owner_id: &str,
    severity: &str,
    message: &str,
    details: Option<Value>,
) -> LogEntry {
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "quality".to_string(),
        message: message.to_string(),
        details: details.and_then(|value| value.as_object().cloned()),
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        goal_id: Some(owner_id.to_string()),
    }
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
