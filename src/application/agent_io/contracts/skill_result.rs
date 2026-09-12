//! Refine reads the agent's decision; supporting material is optional context.
use crate::model::automation::SkillResult;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

#[cfg(test)]
pub(crate) fn result_contract(invocation_id: &str, binding_id: &str, role: &str) -> Value {
    json!({"invocation_id":invocation_id,"binding_id":binding_id,"role":role,
        "outcome":"success","summary":"Your decision and any context useful to the next agent"})
}

impl crate::application::agent_io::structured_output::Contract for SkillResult {
    const LABEL: &'static str = "Skill completion result";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["skill_result", "result"];
    fn example() -> Self {
        Self {
            invocation_id: "invocation".into(),
            binding_id: "binding".into(),
            role: "task".into(),
            outcome: "success".into(),
            summary: String::new(),
            evidence: Vec::new(),
            artifacts: Value::Null,
        }
    }
}

/// Provider-authored content. Routing identity belongs to the host invocation,
/// not to text the model must transcribe from its prompt.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillReport {
    pub outcome: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub artifacts: Value,
}

impl crate::application::agent_io::structured_output::Contract for SkillReport {
    const LABEL: &'static str = "Skill completion report";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["skill_result", "result"];
    fn example() -> Self {
        Self {
            outcome: "success".into(),
            summary: "Your decision and any context useful to the next agent".into(),
            evidence: Vec::new(),
            artifacts: Value::Null,
        }
    }
}

pub(crate) fn report_contract() -> Value {
    use crate::application::agent_io::structured_output::Contract;
    serde_json::to_value(SkillReport::example()).expect("Skill report example must serialize")
}

pub(crate) fn decode_result(
    output: &str,
    invocation_id: &str,
    binding_id: &str,
    role: &str,
    host_bound: bool,
) -> crate::error::RefineResult<SkillResult> {
    use crate::application::agent_io::structured_output::Contract;
    use crate::error::RefineError;
    let result = if let Some(report) = host_bound
        .then(|| SkillReport::decode(output).ok())
        .flatten()
    {
        SkillResult {
            invocation_id: invocation_id.into(),
            binding_id: binding_id.into(),
            role: role.into(),
            outcome: report.outcome,
            summary: report.summary,
            evidence: report.evidence,
            artifacts: report.artifacts,
        }
    } else {
        // Retained receipts from the old contract must still match exactly.
        // Never silently correct a supplied identity, including partial IDs.
        SkillResult::decode(output).map_err(|e| RefineError::Serialization(e.to_string()))?
    };
    result
        .validate(invocation_id, binding_id, role)
        .map_err(RefineError::Serialization)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_binds_report_identity_without_changing_the_decision() {
        crate::application::agent_io::structured_output::assert_contract_roundtrip::<SkillReport>();
        let contract = report_contract();
        for field in ["invocation_id", "binding_id", "role"] {
            assert!(contract.get(field).is_none());
        }
        for outcome in ["success", "failure", "error"] {
            let raw = json!({"outcome":outcome,"summary":"Observed result","evidence":["retained"],"artifacts":{"context":42}}).to_string();
            let result =
                decode_result(&raw, "host-invocation", "host-binding", "quality", true).unwrap();
            result
                .validate("host-invocation", "host-binding", "quality")
                .unwrap();
            assert_eq!(result.outcome, outcome);
            assert_eq!(result.summary, "Observed result");
            assert_eq!(result.evidence, ["retained"]);
            assert_eq!(result.artifacts, json!({"context":42}));
            assert!(
                decode_result(&raw, "host-invocation", "host-binding", "quality", false).is_err()
            );
        }
    }

    #[test]
    fn explicit_legacy_identity_is_never_rebound_or_partially_accepted() {
        let original = result_contract("host-invocation", "host-binding", "quality");
        for host_bound in [false, true] {
            assert!(
                decode_result(
                    &original.to_string(),
                    "host-invocation",
                    "host-binding",
                    "quality",
                    host_bound
                )
                .is_ok()
            );
            for field in ["invocation_id", "binding_id", "role"] {
                let mut wrong = original.clone();
                wrong[field] = json!("different");
                assert!(
                    decode_result(
                        &wrong.to_string(),
                        "host-invocation",
                        "host-binding",
                        "quality",
                        host_bound
                    )
                    .is_err()
                );
                wrong.as_object_mut().unwrap().remove(field);
                assert!(
                    decode_result(
                        &wrong.to_string(),
                        "host-invocation",
                        "host-binding",
                        "quality",
                        host_bound
                    )
                    .is_err()
                );
            }
        }
        assert!(decode_result(r#"{"outcome":"maybe"}"#, "i", "b", "quality", true).is_err());
    }
}
