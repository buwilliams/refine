//! The wire contract a Quality evaluation agent must return. These types exist
//! only at the agent boundary: the prompt's JSON example renders from
//! [`Contract::example`], and the agent's response decodes back through the
//! same types before the fail-safe per-test coercion runs on typed data.

use serde::{Deserialize, Serialize};

use crate::application::agent_io::structured_output::Contract;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityEvaluationWire {
    #[serde(default)]
    pub(crate) ok: Option<bool>,
    #[serde(default)]
    pub(crate) summary: String,
    pub(crate) results: Vec<QualityTestResultWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityTestResultWire {
    pub(crate) test: String,
    // Free-form on the wire: invalid statuses coerce the result to failed with
    // a diagnostic instead of failing the whole evaluation.
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) evidence: String,
    #[serde(default)]
    pub(crate) command: String,
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
    use crate::application::agent_io::structured_output::assert_contract_roundtrip;

    #[test]
    fn quality_evaluation_contract_example_roundtrips() {
        assert_contract_roundtrip::<QualityEvaluationWire>();
    }
}
