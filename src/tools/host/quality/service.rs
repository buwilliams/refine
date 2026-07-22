use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::log::LogEntry;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};

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
            configured: !stored.tests.is_empty(),
            business_requirements: stored.business_requirements,
            instructions: stored.instructions,
            tests: stored.tests,
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
        if let Some(tests) = patch.tests {
            stored.tests = normalize_tests(tests);
        }
        // Retain the legacy field in the wire shape, but Quality is always active for Goal
        // candidates. Older clients may still send `enabled`; it no longer disables evaluation.
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
struct StoredQualitySettings {
    business_requirements: String,
    instructions: String,
    tests: Vec<String>,
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
            tests: normalize_tests(self.tests),
            enabled: "1".to_string(),
            timing: normalize_timing_lossy(&self.timing),
        }
    }
}

impl Default for StoredQualitySettings {
    fn default() -> Self {
        Self {
            business_requirements: String::new(),
            instructions: DEFAULT_INSTRUCTIONS.to_string(),
            tests: Vec::new(),
            enabled: "1".to_string(),
            timing: PRE_MERGE.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckRequest {
    pub owner_id: String,
    pub provider: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub process_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityTestResult {
    pub test: String,
    pub status: String,
    pub evidence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckResult {
    pub owner_id: String,
    pub ok: bool,
    pub summary: String,
    pub results: Vec<QualityTestResult>,
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
                Some(json!({
                    "provider": request.provider,
                    "cwd": request.cwd
                })),
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
                    "summary": &result.summary,
                    "results": &result.results,
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
        let settings = self.load_settings()?;
        if settings.tests.is_empty() {
            return Ok(QualityCheckResult {
                owner_id: request.owner_id,
                ok: true,
                summary: "No Quality tests configured.".to_string(),
                results: Vec::new(),
                diagnostics: vec![
                    "No Quality tests configured; evaluation was a no-op.".to_string(),
                ],
            });
        }
        let tests_json = serde_json::to_string_pretty(&settings.tests).map_err(|error| {
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
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| self.refine_dir.join("runtime/agents"));
        let output = HostAgentProviderService::with_runtime_root(runtime_root).invoke(
            ProviderInvocation {
                provider: request.provider,
                prompt,
                session_id: None,
                cwd: Some(request.cwd),
                process_metadata: request.process_metadata,
            },
        )?;
        parse_quality_provider_output(&request.owner_id, &settings.tests, &output)
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
            summary: if ok {
                "Artifacts match exactly.".to_string()
            } else {
                "Artifacts differ.".to_string()
            },
            results: Vec::new(),
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
        Ok(QualityCheckResult {
            owner_id: owner_id.to_string(),
            ok: true,
            summary: "Quality evaluates every Goal candidate.".to_string(),
            results: Vec::new(),
            diagnostics: vec![format!(
                "Quality is always active; {} plain-text test(s) are configured.",
                settings.tests.len()
            )],
        })
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
    let mut all_passed = true;
    for test in configured_tests {
        let matches = returned
            .iter()
            .filter(|item| item.get("test").and_then(Value::as_str) == Some(test.as_str()))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            all_passed = false;
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
        let valid_status = matches!(status.as_str(), "passed" | "failed");
        let passed = status == "passed";
        all_passed &= valid_status && passed;
        let evidence = item
            .get("evidence")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !valid_status {
            diagnostics.push(format!(
                "Quality agent returned invalid status {status:?} for {test:?}."
            ));
        } else if evidence.is_empty() {
            all_passed = false;
            diagnostics.push(format!("Quality agent returned no evidence for {test:?}."));
        }
        results.push(QualityTestResult {
            test: test.clone(),
            status: if valid_status {
                status
            } else {
                "failed".to_string()
            },
            evidence: if evidence.is_empty() {
                "No evidence was reported.".to_string()
            } else {
                evidence
            },
            command: item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
        });
    }
    if returned.len() != configured_tests.len() {
        all_passed = false;
        diagnostics.push(format!(
            "Quality agent returned {} result(s) for {} configured test(s).",
            returned.len(),
            configured_tests.len()
        ));
    }
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        all_passed = false;
    }
    let summary = value
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            if all_passed {
                "All Quality tests passed.".to_string()
            } else {
                "One or more Quality tests failed.".to_string()
            }
        });
    diagnostics.push(output.trim().to_string());
    Ok(QualityCheckResult {
        owner_id: owner_id.to_string(),
        ok: all_passed,
        summary,
        results,
        diagnostics,
    })
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
        goal_id: owner_id
            .strip_prefix("GOAL")
            .map(|_| owner_id.to_string())
            .or_else(|| owner_id.strip_prefix("goal:").map(ToString::to_string)),
    }
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
