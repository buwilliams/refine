use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::Timestamp;

pub const QUALITY_PROOF_SCHEMA_VERSION: u32 = 1;

/// Durable, provider-independent proof for one exact Quality evaluation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityProof {
    pub schema_version: u32,
    pub goal_id: String,
    /// Zero-based synchronized Goal Round.
    pub round_idx: usize,
    pub evaluation_scope: String,
    pub operation_id: String,
    pub checked_candidate_commit: String,
    pub source_candidate_commit: String,
    pub state: String,
    pub checked_at: Timestamp,
    #[serde(default)]
    pub results: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Box<QualitySkillEvidence>>,
}

/// Coverage of the requirements selected for this exact candidate evaluation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualitySkillEvidence {
    pub requirements_id: String,
    pub required_bindings: Vec<String>,
    pub invocations: std::collections::BTreeMap<String, String>,
}
impl QualitySkillEvidence {
    pub fn covers(&self, snapshot: &Value) -> bool {
        snapshot["id"].as_str() == Some(self.requirements_id.as_str())
            && snapshot["required_bindings"] == serde_json::json!(self.required_bindings)
            && self.required_bindings.len() == self.invocations.len()
            && self
                .required_bindings
                .iter()
                .all(|key| self.invocations.get(key).is_some_and(|id| !id.is_empty()))
    }
}
