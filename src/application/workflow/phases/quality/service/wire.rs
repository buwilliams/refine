//! Compatibility response format. Only the agent decision is required.

use serde::{Deserialize, Serialize};

use crate::application::agent_io::structured_output::Contract;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityEvaluationWire {
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) results: Vec<QualityTestResultWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityTestResultWire {
    #[serde(default)]
    pub(crate) test: String,
    // Optional report text is retained without grading it.
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
            ok: true,
            summary: "result".to_string(),
            results: Vec::new(),
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
