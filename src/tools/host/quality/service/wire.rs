//! The wire contract a Quality evaluation agent must return. These types exist
//! only at the agent boundary: the prompt's JSON example renders from
//! [`Contract::example`], and the agent's response decodes back through the
//! same types before the fail-safe per-test coercion runs on typed data.

use serde::{Deserialize, Serialize};

use crate::structured_output::Contract;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QualityEvaluationWire {
    #[serde(default)]
    pub(super) ok: Option<bool>,
    #[serde(default)]
    pub(super) summary: String,
    pub(super) results: Vec<QualityTestResultWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QualityTestResultWire {
    pub(super) test: String,
    // Free-form on the wire: invalid statuses coerce the result to failed with
    // a diagnostic instead of failing the whole evaluation.
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) evidence: String,
    #[serde(default)]
    pub(super) command: String,
}

impl Contract for QualityEvaluationWire {
    const LABEL: &'static str = "Quality evaluation JSON";

    fn example() -> Self {
        QualityEvaluationWire {
            ok: Some(true),
            summary: "result".to_string(),
            results: vec![QualityTestResultWire {
                test: "exact test".to_string(),
                status: "passed|failed".to_string(),
                evidence: "proof".to_string(),
                command: "non-interactive shell command".to_string(),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured_output::assert_contract_roundtrip;

    #[test]
    fn quality_evaluation_contract_example_roundtrips() {
        assert_contract_roundtrip::<QualityEvaluationWire>();
    }
}
