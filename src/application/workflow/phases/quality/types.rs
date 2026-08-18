use serde::{Deserialize, Serialize};

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
    pub configured: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualitySettingsPatch {
    pub business_requirements: Option<String>,
    pub instructions: Option<String>,
    pub tests: Option<Vec<String>>,
    pub enabled: Option<serde_json::Value>,
    /// Legacy input alias. It is accepted and discarded; Quality always precedes Governance.
    #[serde(default, skip_serializing)]
    pub timing: Option<String>,
}
