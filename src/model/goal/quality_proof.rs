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
}
