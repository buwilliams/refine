use serde::{Deserialize, Serialize};

pub const DEFAULT_INSTRUCTIONS: &str = concat!(
    "Execute the target-app tests for this Goal, if none exist, then write them. ",
    "Write tests that check how the Goal is supposed to work, not based on how ",
    "it is implemented. Failing tests are good when they show true failures. ",
    "Run the minimal number of tests needed to cover the Goal."
);
pub const PRE_MERGE: &str = "pre_merge";
pub const POST_BUILD: &str = "post_build";
pub const SETTINGS_FILE: &str = "quality/settings.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettings {
    pub business_requirements: String,
    pub instructions: String,
    pub enabled: String,
    pub timing: String,
    pub configured: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySettingsPatch {
    pub business_requirements: Option<String>,
    pub instructions: Option<String>,
    pub enabled: Option<serde_json::Value>,
    pub timing: Option<String>,
}
