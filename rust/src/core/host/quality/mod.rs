use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::supervisor::config::{ConfigService, FileSettingsService};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::jobs::{FileJobRegistry, JobHandle, JobRegistry, JobState};
use crate::model::log::LogEntry;

pub const DEFAULT_INSTRUCTIONS: &str = concat!(
    "Execute the e2e tests for this Gap, if none exist, then write them. ",
    "Write tests that check how the Gap is supposed to work, not based on how ",
    "it is implemented. Failing tests are good when they show true failures. ",
    "Certain test frameworks - especially e2e browser-based tests - are very ",
    "time and resource heavy and therefore costly. Run the minimal number of ",
    "tests to cover the Gap."
);
pub const PRE_MERGE: &str = "pre_merge";
pub const POST_REBUILD: &str = "post_rebuild";
pub const SETTINGS_FILE: &str = "quality/settings.json";
pub const REGRESSION_MANIFEST_FILE: &str = "regressions/manifest.json";
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_VIEWPORT_WIDTH: u64 = 1440;
pub const DEFAULT_VIEWPORT_HEIGHT: u64 = 900;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettings {
    pub business_requirements: String,
    pub instructions: String,
    pub enabled: String,
    pub timing: String,
    pub regressions_enabled: String,
    pub regressions: Vec<RegressionCheck>,
    pub configured: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettingsPatch {
    pub business_requirements: Option<String>,
    pub instructions: Option<String>,
    pub enabled: Option<serde_json::Value>,
    pub timing: Option<String>,
    pub regressions_enabled: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegressionCheck {
    pub id: String,
    pub title: String,
    pub description: String,
    pub enabled: bool,
    pub spec_path: String,
    pub viewport: RegressionViewport,
    pub wait_until: String,
    pub timeout_seconds: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_run: Option<RegressionRunSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegressionViewport {
    pub width: u64,
    pub height: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegressionRunSummary {
    pub id: String,
    pub title: String,
    pub run_id: String,
    pub ok: bool,
    pub infra: bool,
    pub message: String,
    pub started_at: String,
    pub finished_at: String,
    pub command: String,
    pub screenshot_path: String,
    pub summary_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_report_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_tail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_data_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RegressionRunResult {
    pub enabled: bool,
    pub ok: bool,
    pub infra: bool,
    pub runs: Vec<RegressionRunSummary>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StoredQualitySettings {
    business_requirements: String,
    instructions: String,
    enabled: String,
    timing: String,
    regressions_enabled: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RegressionManifest {
    schema_version: u64,
    regressions: Vec<RegressionCheck>,
}

#[derive(Clone, Debug)]
pub struct FileQualityService {
    pub durable_root: PathBuf,
}

impl FileQualityService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
        }
    }

    pub fn load_settings(&self) -> RefineResult<QualitySettings> {
        let stored = self.read_stored_settings()?;
        let mut regressions = self.list_regressions(false)?;
        for regression in &mut regressions {
            regression.latest_run = self.latest_run(&regression.id)?;
        }
        Ok(QualitySettings {
            configured: !stored.business_requirements.trim().is_empty()
                && !stored.instructions.trim().is_empty(),
            business_requirements: stored.business_requirements,
            instructions: stored.instructions,
            enabled: stored.enabled,
            timing: stored.timing,
            regressions_enabled: stored.regressions_enabled,
            regressions,
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
        if let Some(enabled) = patch.regressions_enabled {
            stored.regressions_enabled = boolish_setting(&enabled);
        }
        self.write_stored_settings(&stored)?;
        self.load_settings()
    }

    pub fn create_regression(
        &self,
        title: &str,
        description: &str,
        prompt: &str,
    ) -> RefineResult<RegressionCheck> {
        let mut manifest = self.load_manifest()?;
        let existing = manifest
            .regressions
            .iter()
            .map(|regression| regression.id.clone())
            .collect::<BTreeSet<_>>();
        let clean_title = if title.trim().is_empty() {
            prompt.trim().chars().take(80).collect::<String>()
        } else {
            title.trim().to_string()
        };
        let title = if clean_title.trim().is_empty() {
            "Untitled regression".to_string()
        } else {
            clean_title.chars().take(160).collect()
        };
        let id = new_regression_id(&title, &existing);
        let now = now_timestamp();
        let regression = RegressionCheck {
            id: id.clone(),
            title: title.clone(),
            description: if description.trim().is_empty() {
                prompt.trim().to_string()
            } else {
                description.trim().to_string()
            },
            enabled: true,
            spec_path: format!("specs/{id}.js"),
            viewport: default_viewport(),
            wait_until: "networkidle".to_string(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            created_at: now.clone(),
            updated_at: now,
            latest_run: None,
        };
        fs::create_dir_all(self.regression_specs_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create regression specs directory: {error}"
            ))
        })?;
        fs::write(
            self.spec_file(&regression),
            regression_spec_template(&regression, prompt),
        )
        .map_err(|error| RefineError::Io(format!("failed to write regression spec: {error}")))?;
        manifest.regressions.push(regression.clone());
        self.save_manifest(&manifest)?;
        Ok(regression)
    }

    pub fn update_regression(
        &self,
        regression_id: &str,
        updates: &serde_json::Value,
    ) -> RefineResult<RegressionCheck> {
        let mut manifest = self.load_manifest()?;
        let Some(position) = manifest
            .regressions
            .iter()
            .position(|regression| regression.id == regression_id)
        else {
            return Err(RefineError::NotFound("Regression not found".to_string()));
        };
        let mut regression = manifest.regressions[position].clone();
        if let Some(title) = updates.get("title").and_then(|value| value.as_str()) {
            regression.title = title.trim().chars().take(160).collect();
            if regression.title.is_empty() {
                regression.title = "Untitled regression".to_string();
            }
        }
        if let Some(description) = updates.get("description").and_then(|value| value.as_str()) {
            regression.description = description.trim().to_string();
        }
        if let Some(enabled) = updates.get("enabled") {
            regression.enabled = value_is_truthy(enabled);
        }
        if let Some(wait_until) = updates.get("wait_until").and_then(|value| value.as_str()) {
            regression.wait_until = wait_until.trim().to_string();
            if regression.wait_until.is_empty() {
                regression.wait_until = "networkidle".to_string();
            }
        }
        if let Some(timeout) = updates
            .get("timeout_seconds")
            .and_then(|value| value.as_u64())
        {
            if timeout > 0 {
                regression.timeout_seconds = timeout;
            }
        }
        if let Some(viewport) = updates.get("viewport").and_then(|value| value.as_object()) {
            let width = viewport
                .get("width")
                .and_then(|value| value.as_u64())
                .filter(|value| *value > 0)
                .unwrap_or(regression.viewport.width);
            let height = viewport
                .get("height")
                .and_then(|value| value.as_u64())
                .filter(|value| *value > 0)
                .unwrap_or(regression.viewport.height);
            regression.viewport = RegressionViewport { width, height };
        }
        regression.updated_at = now_timestamp();
        validate_regression(&regression)?;
        manifest.regressions[position] = regression.clone();
        self.save_manifest(&manifest)?;
        Ok(regression)
    }

    pub fn delete_regression(&self, regression_id: &str) -> RefineResult<()> {
        let mut manifest = self.load_manifest()?;
        let original_len = manifest.regressions.len();
        manifest
            .regressions
            .retain(|regression| regression.id != regression_id);
        if manifest.regressions.len() == original_len {
            return Err(RefineError::NotFound("Regression not found".to_string()));
        }
        self.save_manifest(&manifest)?;
        let spec_path = self
            .regression_specs_dir()
            .join(format!("{regression_id}.js"));
        match fs::remove_file(&spec_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to delete regression spec {}: {error}",
                    spec_path.display()
                )));
            }
        }
        Ok(())
    }

    pub fn run_regressions(&self, only_enabled: bool) -> RefineResult<RegressionRunResult> {
        let settings = self.read_stored_settings()?;
        if settings.regressions_enabled != "1" {
            return Ok(RegressionRunResult {
                enabled: false,
                ok: true,
                infra: false,
                runs: Vec::new(),
                message: "Regression checks are disabled.".to_string(),
            });
        }
        let mut regressions = self.list_regressions(false)?;
        if only_enabled {
            regressions.retain(|regression| regression.enabled);
        }
        if regressions.is_empty() {
            return Ok(RegressionRunResult {
                enabled: true,
                ok: true,
                infra: false,
                runs: Vec::new(),
                message: "No regression checks configured.".to_string(),
            });
        }
        let mut runs = Vec::new();
        for regression in regressions {
            runs.push(self.record_regression_run(&regression)?);
        }
        let passed = runs.iter().filter(|run| run.ok).count();
        let total = runs.len();
        Ok(RegressionRunResult {
            enabled: true,
            ok: passed == total,
            infra: runs.iter().any(|run| run.infra),
            runs,
            message: format!("{passed}/{total} regression checks passed"),
        })
    }

    pub fn list_regressions(&self, include_latest: bool) -> RefineResult<Vec<RegressionCheck>> {
        let mut regressions = self.load_manifest()?.regressions;
        regressions.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        if include_latest {
            for regression in &mut regressions {
                regression.latest_run = self.latest_run(&regression.id)?;
            }
        } else {
            for regression in &mut regressions {
                regression.latest_run = None;
            }
        }
        Ok(regressions)
    }

    fn record_regression_run(
        &self,
        regression: &RegressionCheck,
    ) -> RefineResult<RegressionRunSummary> {
        let run_id = new_run_id();
        let out_dir = self
            .regression_runs_dir()
            .join(&regression.id)
            .join(&run_id);
        fs::create_dir_all(&out_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create regression run directory {}: {error}",
                out_dir.display()
            ))
        })?;
        let summary_path = out_dir.join("summary.json");
        let spec_exists = self.spec_file(regression).exists();
        let started_at = now_timestamp();
        let screenshot_path = out_dir.join("screenshot.png");
        let json_report_path = out_dir.join("report.json");
        let mut command = format!("playwright test {}", regression.spec_path);
        let mut stdout_tail = None;
        let mut stderr_tail = None;
        let mut json_report = None;
        let (ok, infra, message) = if !spec_exists {
            (false, true, "regression spec is missing".to_string())
        } else {
            let target_url = self.target_app_url();
            if target_url.trim().is_empty() {
                (
                    false,
                    true,
                    "target app URL is required to run regression checks".to_string(),
                )
            } else {
                match self.run_playwright(regression, &out_dir, &screenshot_path, &target_url) {
                    Ok(execution) => {
                        command = execution.command;
                        stdout_tail = execution.stdout_tail;
                        stderr_tail = execution.stderr_tail;
                        if execution.json_report_written {
                            json_report = Some(json_report_path.to_string_lossy().to_string());
                        }
                        (execution.ok, execution.infra, execution.message)
                    }
                    Err(error) => (false, true, error.to_string()),
                }
            }
        };
        let finished_at = now_timestamp();
        let summary = RegressionRunSummary {
            id: regression.id.clone(),
            title: regression.title.clone(),
            run_id,
            ok,
            infra,
            message,
            started_at,
            finished_at,
            command,
            screenshot_path: if screenshot_path.exists() {
                screenshot_path.to_string_lossy().to_string()
            } else {
                String::new()
            },
            summary_path: summary_path.to_string_lossy().to_string(),
            json_report_path: json_report,
            stdout_tail,
            stderr_tail,
            screenshot_data_url: None,
        };
        let encoded = serde_json::to_string_pretty(&summary).map_err(|error| {
            RefineError::Serialization(format!("failed to encode regression run: {error}"))
        })?;
        fs::write(&summary_path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write regression run summary {}: {error}",
                summary_path.display()
            ))
        })?;
        Ok(summary)
    }

    fn run_playwright(
        &self,
        regression: &RegressionCheck,
        out_dir: &Path,
        screenshot_path: &Path,
        target_url: &str,
    ) -> RefineResult<RegressionExecution> {
        let Some(mut command) = self.playwright_command() else {
            return Ok(RegressionExecution {
                command: format!("playwright test {}", regression.spec_path),
                ok: false,
                infra: true,
                message: "Playwright CLI was not found; install @playwright/test in the target app or make playwright available on PATH".to_string(),
                stdout_tail: None,
                stderr_tail: None,
                json_report_written: false,
            });
        };
        let command_display = format!(
            "{} test {} --reporter=json --output {} --timeout {}",
            command.display,
            regression.spec_path,
            out_dir.display(),
            regression.timeout_seconds.saturating_mul(1000)
        );
        command
            .command
            .arg("test")
            .arg(self.spec_file(regression))
            .arg("--reporter=json")
            .arg("--output")
            .arg(out_dir)
            .arg("--timeout")
            .arg(regression.timeout_seconds.saturating_mul(1000).to_string())
            .env("REFINE_TARGET_APP_URL", target_url)
            .env("REFINE_REGRESSION_SCREENSHOT", screenshot_path)
            .current_dir(self.project_root());
        let output = command.command.output().map_err(|error| {
            RefineError::Io(format!(
                "failed to run Playwright regression command: {error}"
            ))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let report_path = out_dir.join("report.json");
        let json_report_written = if stdout.trim().is_empty() {
            false
        } else {
            fs::write(&report_path, &stdout).map_err(|error| {
                RefineError::Io(format!(
                    "failed to write Playwright JSON report {}: {error}",
                    report_path.display()
                ))
            })?;
            true
        };
        let ok = output.status.success();
        Ok(RegressionExecution {
            command: command_display,
            ok,
            infra: false,
            message: if ok {
                "Playwright regression passed".to_string()
            } else {
                format!(
                    "Playwright regression failed{}",
                    output
                        .status
                        .code()
                        .map(|code| format!(" with exit code {code}"))
                        .unwrap_or_default()
                )
            },
            stdout_tail: tail_text(&stdout, 4000),
            stderr_tail: tail_text(&stderr, 4000),
            json_report_written,
        })
    }

    fn latest_run(&self, regression_id: &str) -> RefineResult<Option<RegressionRunSummary>> {
        let base = self.regression_runs_dir().join(regression_id);
        if !base.exists() {
            return Ok(None);
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&base).map_err(|error| {
            RefineError::Io(format!(
                "failed to read regression runs directory {}: {error}",
                base.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!("failed to read regression run entry: {error}"))
            })?;
            let summary = entry.path().join("summary.json");
            if summary.is_file() {
                let modified = summary
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok();
                candidates.push((modified, summary));
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        let Some((_, path)) = candidates.into_iter().next() else {
            return Ok(None);
        };
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read regression run summary {}: {error}",
                path.display()
            ))
        })?;
        let mut summary =
            serde_json::from_str::<RegressionRunSummary>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse regression run summary {}: {error}",
                    path.display()
                ))
            })?;
        summary.screenshot_data_url = None;
        Ok(Some(summary))
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

    fn load_manifest(&self) -> RefineResult<RegressionManifest> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(RegressionManifest::default());
        }
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read regression manifest {}: {error}",
                path.display()
            ))
        })?;
        let raw = serde_json::from_str::<RegressionManifest>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse regression manifest {}: {error}",
                path.display()
            ))
        })?;
        raw.normalized()
    }

    fn save_manifest(&self, manifest: &RegressionManifest) -> RefineResult<()> {
        let manifest = manifest.clone().normalized()?;
        let path = self.manifest_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create regression manifest directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let encoded = serde_json::to_string_pretty(&manifest).map_err(|error| {
            RefineError::Serialization(format!("failed to encode regression manifest: {error}"))
        })?;
        fs::write(&path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write regression manifest {}: {error}",
                path.display()
            ))
        })
    }

    fn settings_path(&self) -> PathBuf {
        self.durable_root.join(SETTINGS_FILE)
    }

    fn manifest_path(&self) -> PathBuf {
        self.durable_root.join(REGRESSION_MANIFEST_FILE)
    }

    fn regression_specs_dir(&self) -> PathBuf {
        self.durable_root.join("regressions/specs")
    }

    fn regression_runs_dir(&self) -> PathBuf {
        self.durable_root.join("regressions/runs")
    }

    fn spec_file(&self, regression: &RegressionCheck) -> PathBuf {
        self.durable_root
            .join("regressions")
            .join(&regression.spec_path)
    }

    fn project_root(&self) -> PathBuf {
        self.durable_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.durable_root.clone())
    }

    fn target_app_url(&self) -> String {
        FileSettingsService::new(&self.durable_root)
            .load()
            .ok()
            .and_then(|settings| {
                settings
                    .get("target_app_url")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .or_else(|| std::env::var("REFINE_TARGET_APP_URL").ok())
            .unwrap_or_default()
    }

    fn playwright_command(&self) -> Option<PlaywrightCommand> {
        let project_root = self.project_root();
        let local = project_root
            .join("node_modules")
            .join(".bin")
            .join(if cfg!(windows) {
                "playwright.cmd"
            } else {
                "playwright"
            });
        if local.is_file() {
            return Some(PlaywrightCommand {
                display: local.display().to_string(),
                command: Command::new(local),
            });
        }
        if project_root.join("package.json").is_file() {
            let mut command = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });
            command.arg("playwright");
            return Some(PlaywrightCommand {
                display: "npx playwright".to_string(),
                command,
            });
        }
        Some(PlaywrightCommand {
            display: "playwright".to_string(),
            command: Command::new(if cfg!(windows) {
                "playwright.cmd"
            } else {
                "playwright"
            }),
        })
    }
}

#[derive(Debug)]
struct PlaywrightCommand {
    display: String,
    command: Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegressionExecution {
    command: String,
    ok: bool,
    infra: bool,
    message: String,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
    json_report_written: bool,
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
            regressions_enabled: boolish_string(&self.regressions_enabled),
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
            regressions_enabled: "0".to_string(),
        }
    }
}

impl RegressionManifest {
    fn normalized(self) -> RefineResult<Self> {
        let mut regressions = Vec::new();
        for regression in self.regressions {
            if validate_regression(&regression).is_ok() {
                regressions.push(RegressionCheck {
                    latest_run: None,
                    ..regression
                });
            }
        }
        Ok(Self {
            schema_version: 1,
            regressions,
        })
    }
}

impl Default for RegressionManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            regressions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckRequest {
    pub owner_id: String,
    pub command: String,
    pub browser_required: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckResult {
    pub owner_id: String,
    pub ok: bool,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityJobResult {
    pub job: JobHandle,
    pub result: QualityCheckResult,
}

pub trait QualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult>;
    fn browser_qa(&self, owner_id: &str) -> RefineResult<QualityCheckResult>;
    fn regressions(&self, owner_id: &str) -> RefineResult<QualityCheckResult>;
    fn screenshots(&self, owner_id: &str) -> RefineResult<Vec<String>>;
    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult>;
    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult>;
}

#[derive(Clone, Debug)]
pub struct QualityJobRunner {
    pub durable_root: PathBuf,
    pub runtime_root: PathBuf,
}

impl QualityJobRunner {
    pub fn new(durable_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityJobResult> {
        let registry = FileJobRegistry::new(&self.runtime_root);
        let job = registry.register(&format!("quality:{}", request.owner_id))?;
        registry.append_log(
            &job.id,
            quality_job_log(
                &request.owner_id,
                "info",
                "Quality checks started",
                Some(json!({
                    "command": request.command,
                    "browser_required": request.browser_required
                })),
            ),
        )?;
        let service = FileQualityService::new(&self.durable_root);
        let result = service.run_checks(request)?;
        registry.append_log(
            &job.id,
            quality_job_log(
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
        let job = registry.finish(
            &job.id,
            if result.ok {
                JobState::Succeeded
            } else {
                JobState::Failed
            },
        )?;
        Ok(QualityJobResult { job, result })
    }
}

impl QualityService for FileQualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult> {
        if request.command.trim().is_empty() {
            return if request.browser_required {
                self.browser_qa(&request.owner_id)
            } else {
                Ok(QualityCheckResult {
                    owner_id: request.owner_id,
                    ok: true,
                    diagnostics: vec!["No quality command configured.".to_string()],
                })
            };
        }
        let output = shell_command(&request.command)
            .current_dir(self.project_root())
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run quality command: {error}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let mut diagnostics = Vec::new();
        if let Some(stdout) = tail_text(&stdout, 4000) {
            diagnostics.push(stdout);
        }
        if let Some(stderr) = tail_text(&stderr, 4000) {
            diagnostics.push(stderr);
        }
        if diagnostics.is_empty() {
            diagnostics.push(format!("quality command exited {}", output.status));
        }
        Ok(QualityCheckResult {
            owner_id: request.owner_id,
            ok: output.status.success(),
            diagnostics,
        })
    }

    fn browser_qa(&self, owner_id: &str) -> RefineResult<QualityCheckResult> {
        self.regressions(owner_id)
    }

    fn regressions(&self, owner_id: &str) -> RefineResult<QualityCheckResult> {
        let result = self.run_regressions(true)?;
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: result.ok,
            diagnostics: result
                .runs
                .into_iter()
                .map(|run| format!("{}: {}", run.title, run.message))
                .collect(),
        })
    }

    fn screenshots(&self, _owner_id: &str) -> RefineResult<Vec<String>> {
        let mut paths = Vec::new();
        for regression in self.list_regressions(true)? {
            if let Some(run) = regression.latest_run {
                if !run.screenshot_path.trim().is_empty()
                    && Path::new(&run.screenshot_path).is_file()
                {
                    paths.push(run.screenshot_path);
                }
            }
        }
        paths.sort();
        Ok(paths)
    }

    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult> {
        let baseline_path = PathBuf::from(baseline);
        let candidate_path = PathBuf::from(candidate);
        let baseline_bytes = fs::read(&baseline_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read baseline screenshot {}: {error}",
                baseline_path.display()
            ))
        })?;
        let candidate_bytes = fs::read(&candidate_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read candidate screenshot {}: {error}",
                candidate_path.display()
            ))
        })?;
        let ok = baseline_bytes == candidate_bytes;
        Ok(QualityCheckResult {
            owner_id: format!("{baseline}:{candidate}"),
            ok,
            diagnostics: vec![if ok {
                "screenshots match exactly".to_string()
            } else {
                format!(
                    "screenshots differ: baseline {} bytes, candidate {} bytes",
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
        let regressions = self.regressions(owner_id)?;
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: regressions.ok,
            diagnostics: if regressions.diagnostics.is_empty() {
                vec!["Quality gate completed with no regression diagnostics.".to_string()]
            } else {
                regressions.diagnostics
            },
        })
    }
}

pub fn quality_settings_value(settings: &QualitySettings) -> serde_json::Value {
    json!(settings)
}

fn validate_regression(regression: &RegressionCheck) -> RefineResult<()> {
    if !valid_regression_id(&regression.id) {
        return Err(RefineError::InvalidInput(
            "regression id must match [a-z0-9][a-z0-9_-]{1,63}".to_string(),
        ));
    }
    if regression.spec_path != format!("specs/{}.js", regression.id) {
        return Err(RefineError::InvalidInput(
            "regression spec_path must stay inside regressions/specs".to_string(),
        ));
    }
    if regression.viewport.width == 0 || regression.viewport.height == 0 {
        return Err(RefineError::InvalidInput(
            "regression viewport dimensions must be positive".to_string(),
        ));
    }
    Ok(())
}

fn valid_regression_id(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    let len = 1 + chars.clone().count();
    if !(2..=64).contains(&len) {
        return false;
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

fn new_regression_id(title: &str, existing: &BTreeSet<String>) -> String {
    let slug = slugify(title);
    let mut candidate = if slug.is_empty() {
        "regression".to_string()
    } else {
        slug.chars().take(36).collect::<String>()
    };
    if candidate.len() < 2 {
        candidate.push('1');
    }
    let base = candidate.clone();
    let mut suffix = 1;
    while existing.contains(&candidate) || !valid_regression_id(&candidate) {
        suffix += 1;
        candidate = format!("{base}-{suffix}");
    }
    candidate
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.to_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn regression_spec_template(regression: &RegressionCheck, prompt: &str) -> String {
    let prompt_line = if prompt.trim().is_empty() {
        String::new()
    } else {
        format!("// Initial prompt: {}\n", prompt.trim().replace('\n', " "))
    };
    format!(
        r#"const {{ test, expect }} = require("@playwright/test");

{prompt_line}test({title:?}, async ({{ page }}) => {{
  const targetUrl = process.env.REFINE_TARGET_APP_URL;
  const screenshotPath = process.env.REFINE_REGRESSION_SCREENSHOT;
  if (!targetUrl) throw new Error("REFINE_TARGET_APP_URL is required");
  if (!screenshotPath) throw new Error("REFINE_REGRESSION_SCREENSHOT is required");

  await page.goto(targetUrl, {{ waitUntil: {wait_until:?} }});
  await expect(page.locator("body")).toBeVisible();
  await page.screenshot({{ path: screenshotPath, fullPage: true }});
}});
"#,
        prompt_line = prompt_line,
        title = regression.title,
        wait_until = regression.wait_until,
    )
}

fn default_viewport() -> RegressionViewport {
    RegressionViewport {
        width: DEFAULT_VIEWPORT_WIDTH,
        height: DEFAULT_VIEWPORT_HEIGHT,
    }
}

fn normalize_timing(value: &str) -> RefineResult<String> {
    match value.trim() {
        PRE_MERGE => Ok(PRE_MERGE.to_string()),
        POST_REBUILD => Ok(POST_REBUILD.to_string()),
        _ => Err(RefineError::InvalidInput(
            "timing must be one of pre_merge, post_rebuild".to_string(),
        )),
    }
}

fn normalize_timing_lossy(value: &str) -> String {
    if value.trim() == POST_REBUILD {
        POST_REBUILD.to_string()
    } else {
        PRE_MERGE.to_string()
    }
}

fn boolish_setting(value: &serde_json::Value) -> String {
    if value_is_truthy(value) {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

fn boolish_string(value: &str) -> String {
    if matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    ) {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

fn value_is_truthy(value: &serde_json::Value) -> bool {
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

fn tail_text(value: &str, max_chars: usize) -> Option<String> {
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

fn quality_job_log(
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

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn new_run_id() -> String {
    format!(
        "{}-{}",
        Utc::now().format("%Y%m%d%H%M%S%3f"),
        std::process::id()
    )
}

#[allow(dead_code)]
fn is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn quality_settings_persist_and_report_configured_state() {
        let temp_root = unique_temp_dir("quality-settings");
        let durable_root = temp_root.join(".refine");
        let service = FileQualityService::new(&durable_root);

        let saved = service
            .save_settings(QualitySettingsPatch {
                business_requirements: Some("Must load dashboard".to_string()),
                instructions: Some("Run focused checks".to_string()),
                enabled: Some(json!("1")),
                timing: Some(POST_REBUILD.to_string()),
                regressions_enabled: Some(json!(true)),
            })
            .unwrap();

        assert_eq!(saved.enabled, "1");
        assert_eq!(saved.timing, POST_REBUILD);
        assert_eq!(saved.regressions_enabled, "1");
        assert!(saved.configured);
        assert!(durable_root.join(SETTINGS_FILE).exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn quality_regressions_create_update_delete_and_run() {
        let temp_root = unique_temp_dir("quality-regressions");
        let durable_root = temp_root.join(".refine");
        write_fake_playwright(&temp_root, 0);
        FileSettingsService::new(&durable_root)
            .update(&json!({"target_app_url": "http://127.0.0.1:3000"}))
            .unwrap();
        let service = FileQualityService::new(&durable_root);
        service
            .save_settings(QualitySettingsPatch {
                regressions_enabled: Some(json!("1")),
                ..QualitySettingsPatch::default()
            })
            .unwrap();

        let created = service
            .create_regression("Dashboard smoke", "Open dashboard", "Navigate home")
            .unwrap();
        assert_eq!(created.id, "dashboard-smoke");
        assert!(
            durable_root
                .join("regressions/specs/dashboard-smoke.js")
                .exists()
        );

        let updated = service
            .update_regression(&created.id, &json!({"enabled": false}))
            .unwrap();
        assert!(!updated.enabled);
        let no_runs = service.run_regressions(true).unwrap();
        assert_eq!(no_runs.message, "No regression checks configured.");

        service
            .update_regression(&created.id, &json!({"enabled": true}))
            .unwrap();
        let result = service.run_regressions(true).unwrap();
        assert!(result.ok);
        assert_eq!(result.runs.len(), 1);
        assert_eq!(result.runs[0].message, "Playwright regression passed");
        assert!(result.runs[0].json_report_path.is_some());
        assert!(result.runs[0].screenshot_path.ends_with("screenshot.png"));

        let loaded = service.load_settings().unwrap();
        assert_eq!(loaded.regressions[0].latest_run.as_ref().unwrap().ok, true);

        service.delete_regression(&created.id).unwrap();
        assert!(service.list_regressions(true).unwrap().is_empty());
        assert!(
            !durable_root
                .join("regressions/specs/dashboard-smoke.js")
                .exists()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn quality_regression_run_reports_missing_target_url_as_infra() {
        let temp_root = unique_temp_dir("quality-regression-infra");
        let durable_root = temp_root.join(".refine");
        write_fake_playwright(&temp_root, 0);
        let service = FileQualityService::new(&durable_root);
        service
            .save_settings(QualitySettingsPatch {
                regressions_enabled: Some(json!(true)),
                ..QualitySettingsPatch::default()
            })
            .unwrap();
        service
            .create_regression("Dashboard smoke", "Open dashboard", "Navigate home")
            .unwrap();

        let result = service.run_regressions(true).unwrap();
        assert!(!result.ok);
        assert!(result.infra);
        assert!(result.runs[0].message.contains("target app URL"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn quality_service_runs_commands_compares_screenshots_and_gates() {
        let temp_root = unique_temp_dir("quality-trait");
        let durable_root = temp_root.join(".refine");
        write_fake_playwright(&temp_root, 0);
        FileSettingsService::new(&durable_root)
            .update(&json!({"target_app_url": "http://127.0.0.1:3000"}))
            .unwrap();
        let service = FileQualityService::new(&durable_root);

        let command_result = service
            .run_checks(QualityCheckRequest {
                owner_id: "GAP1".to_string(),
                command: "printf command-ok".to_string(),
                browser_required: false,
            })
            .unwrap();
        assert!(command_result.ok);
        assert_eq!(command_result.diagnostics, vec!["command-ok"]);

        service
            .save_settings(QualitySettingsPatch {
                enabled: Some(json!(true)),
                regressions_enabled: Some(json!(true)),
                ..QualitySettingsPatch::default()
            })
            .unwrap();
        service
            .create_regression("Dashboard smoke", "Open dashboard", "Navigate home")
            .unwrap();
        let gate = service.gate("GAP1").unwrap();
        assert!(gate.ok);
        let screenshots = service.screenshots("GAP1").unwrap();
        assert_eq!(screenshots.len(), 1);

        let baseline = temp_root.join("baseline.png");
        let candidate = temp_root.join("candidate.png");
        fs::write(&baseline, b"same").unwrap();
        fs::write(&candidate, b"same").unwrap();
        assert!(
            service
                .compare(baseline.to_str().unwrap(), candidate.to_str().unwrap())
                .unwrap()
                .ok
        );
        fs::write(&candidate, b"different").unwrap();
        assert!(
            !service
                .compare(baseline.to_str().unwrap(), candidate.to_str().unwrap())
                .unwrap()
                .ok
        );

        fs::remove_dir_all(temp_root).unwrap();
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

    fn write_fake_playwright(root: &Path, exit_code: i32) {
        let bin_dir = root.join("node_modules/.bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join("playwright");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' '{{\"status\":\"passed\"}}'\nif [ -n \"$REFINE_REGRESSION_SCREENSHOT\" ]; then printf 'png' > \"$REFINE_REGRESSION_SCREENSHOT\"; fi\nexit {exit_code}"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }
}
