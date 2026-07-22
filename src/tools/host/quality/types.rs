use serde::{Deserialize, Serialize};

pub const DEFAULT_INSTRUCTIONS: &str = concat!(
    "Evaluate every Quality test against the Goal candidate. Determine the least ",
    "expensive reliable way to run each test, then report pass or fail with evidence. ",
    "Do not change product code while evaluating the candidate."
);
pub const PRE_MERGE: &str = "pre_merge";
pub const POST_BUILD: &str = "post_build";
pub const SETTINGS_FILE: &str = "quality/settings.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettings {
    pub business_requirements: String,
    pub instructions: String,
    pub tests: Vec<String>,
    pub enabled: String,
    pub timing: String,
    pub configured: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettingsPatch {
    pub business_requirements: Option<String>,
    pub instructions: Option<String>,
    pub tests: Option<Vec<String>>,
    pub enabled: Option<serde_json::Value>,
    pub timing: Option<String>,
}
