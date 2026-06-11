use serde::{Deserialize, Serialize};

pub const DEFAULT_INSTRUCTIONS: &str = concat!(
    "Execute the e2e tests for this Gap, if none exist, then write them. ",
    "Write tests that check how the Gap is supposed to work, not based on how ",
    "it is implemented. Failing tests are good when they show true failures. ",
    "Certain test frameworks - especially e2e browser-based tests - are very ",
    "time and resource heavy and therefore costly. Run the minimal number of ",
    "tests to cover the Gap."
);
pub const PRE_MERGE: &str = "pre_merge";
pub const POST_BUILD: &str = "post_build";
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
