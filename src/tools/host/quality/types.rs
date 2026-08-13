use serde::{Deserialize, Serialize};

pub const PRE_MERGE: &str = "pre_merge";
pub const POST_BUILD: &str = "post_build";
pub const SETTINGS_FILE: &str = "quality/settings.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettings {
    pub business_requirements: String,
    pub instructions: String,
    pub tests: Vec<String>,
    /// Commands imported from formerly authoritative target-app test settings. They remain
    /// enforced until a user explicitly replaces them with plain-text Quality tests.
    pub legacy_commands: Vec<String>,
    pub enabled: String,
    /// Legacy persisted value retained only to read historical configuration.
    #[serde(skip_serializing)]
    pub timing: String,
    pub configured: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettingsPatch {
    pub business_requirements: Option<String>,
    pub instructions: Option<String>,
    pub tests: Option<Vec<String>>,
    pub enabled: Option<serde_json::Value>,
    /// Legacy input alias. New workflow always runs Quality before Governance.
    #[serde(default, skip_serializing)]
    pub timing: Option<String>,
}
