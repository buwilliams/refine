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
/// This is the advertised schema; `decode_result` normalizes provider extensions
/// before decoding the strict persisted result.
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
    use crate::application::agent_io::structured_output::{
        Contract, DecodeOptions, decode_structured,
    };
    use crate::error::RefineError;
    let identity = [
        ("invocation_id", invocation_id),
        ("binding_id", binding_id),
        ("role", role),
    ];
    let result: SkillResult = decode_structured(
        output,
        &DecodeOptions::with_envelopes(SkillReport::LABEL, SkillReport::ENVELOPE_FIELDS),
        |value| {
            if let Some(object) = value.as_object_mut() {
                // Ignore provider extensions only at this boundary; persisted
                // SkillResult records keep their strict schema.
                object.retain(|key, _| {
                    matches!(
                        key.as_str(),
                        "invocation_id"
                            | "binding_id"
                            | "role"
                            | "outcome"
                            | "summary"
                            | "evidence"
                            | "artifacts"
                    )
                });
                // Providers may return structured supporting evidence. Preserve
                // each value as compact JSON text while keeping strings verbatim
                // and the persisted Vec<String> schema strict. Non-arrays still fail.
                if let Some(Value::Array(evidence)) = object.get_mut("evidence") {
                    for entry in evidence {
                        if !entry.is_string() {
                            *entry = Value::String(entry.to_string());
                        }
                    }
                }
                // Any supplied identity selects the legacy path: all three
                // fields must deserialize and match, even for new receipts.
                if host_bound && !identity.iter().any(|(key, _)| object.contains_key(*key)) {
                    for (key, expected) in identity {
                        object.insert(key.into(), Value::String(expected.into()));
                    }
                }
            }
            Ok(())
        },
    )
    .map_err(|e| RefineError::Serialization(e.to_string()))?;
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
        let mut original = result_contract("host-invocation", "host-binding", "quality");
        original["checklist"] = json!([{"id":"P1"}]);
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
    #[test]
    fn provider_extensions_are_tolerated_across_completion_transports() {
        for outcome in ["success", "failure", "error"] {
            for host_bound in [false, true] {
                let mut value = json!({"outcome":outcome,"summary":"Observed result",
                "evidence":["check\nreported", {"id":"P1","note":"did X"},
                    ["nested", {"result":{"outcome":"failure"}}], 42, 1.5, true, false, null,
                    "{\"outcome\":\"error\"}"],"artifacts":{"checklist":[{"id":"P1"}],
                    "result":{"outcome":"success"},"quoted":r#"{"outcome":"error"}"#},
                "checklist":[{"id":"ignored"}],"extra":{"nested":true}});
                if !host_bound {
                    value["invocation_id"] = json!("i");
                    value["binding_id"] = json!("b");
                    value["role"] = json!("quality");
                }
                let raw = value.to_string();
                for output in [
                    raw.clone(),
                    format!("```json\n{raw}\n```"),
                    format!("Review complete. {raw} Done."),
                    json!({"result":value}).to_string(),
                    json!({"skill_result":raw}).to_string(),
                    serde_json::to_string(&raw).unwrap(),
                    format!(
                        "Review complete. {} Done.",
                        serde_json::to_string(&raw).unwrap()
                    ),
                    format!("Review complete. ```json\n{raw}\n``` Done."),
                    format!("Repeated response: {raw} then {raw}"),
                ] {
                    let result = decode_result(&output, "i", "b", "quality", host_bound).unwrap();
                    result.validate("i", "b", "quality").unwrap();
                    assert_eq!(result.outcome, outcome);
                    assert_eq!(result.summary, "Observed result");
                    let original = value["evidence"].as_array().unwrap();
                    assert_eq!(result.evidence.len(), original.len());
                    for (actual, expected) in result.evidence.iter().zip(original) {
                        if let Some(expected) = expected.as_str() {
                            assert_eq!(actual, expected);
                        } else {
                            assert_eq!(serde_json::from_str::<Value>(actual).unwrap(), *expected);
                            assert_eq!(*actual, expected.to_string());
                        }
                    }
                    assert_eq!(result.artifacts, value["artifacts"]);
                }
                // Tolerance belongs to provider ingestion, not persisted records.
                assert!(serde_json::from_value::<SkillResult>(value).is_err());
            }
        }
    }

    #[test]
    fn object_evidence_is_accepted_only_at_provider_ingestion() {
        let raw = r#"{"outcome":"success","summary":"...","evidence":[{"id":"P1","note":"did X"}],"artifacts":{}}"#;
        let result = decode_result(raw, "i", "b", "implement", true).unwrap();
        assert_eq!(result.evidence, [r#"{"id":"P1","note":"did X"}"#]);
        let mut persisted = serde_json::to_value(&result).unwrap();
        assert!(serde_json::from_value::<SkillResult>(persisted.clone()).is_ok());
        persisted["evidence"] = json!([{"id":"P1","note":"did X"}]);
        assert!(serde_json::from_value::<SkillResult>(persisted).is_err());
        let minimal =
            decode_result(r#"{"outcome":"success"}"#, "i", "b", "implement", true).unwrap();
        assert!(minimal.evidence.is_empty());
    }

    #[test]
    fn malformed_content_keeps_actionable_diagnostics() {
        for (field, value, diagnostic) in [
            ("outcome", json!(42), "outcome"),
            (
                "outcome",
                json!("maybe"),
                "requires success, failure, or error",
            ),
            ("summary", json!([]), "summary"),
            ("evidence", json!("check passed"), "evidence"),
            ("evidence", json!({"note":"check passed"}), "evidence"),
            ("evidence", json!(42), "evidence"),
            ("evidence", json!(true), "evidence"),
            ("evidence", json!(null), "evidence"),
        ] {
            for host_bound in [false, true] {
                let mut report = json!({"outcome":"success","checklist":[]});
                if !host_bound {
                    report["invocation_id"] = json!("i");
                    report["binding_id"] = json!("b");
                    report["role"] = json!("quality");
                }
                report[field] = value.clone();
                let error = decode_result(&report.to_string(), "i", "b", "quality", host_bound)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(diagnostic), "{error}");
            }
        }
        let error = decode_result(r#"{"checklist":[]}"#, "i", "b", "quality", true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing field `outcome`"), "{error}");
        for field in ["invocation_id", "binding_id", "role"] {
            for value in [json!(null), json!(42), json!("i")] {
                let mut report = json!({"outcome":"success","checklist":[]});
                report[field] = value;
                assert!(decode_result(&report.to_string(), "i", "b", "quality", true).is_err());
            }
        }
    }

    #[test]
    fn distinct_completions_and_conflicting_envelopes_remain_ambiguous() {
        for (raw, diagnostic) in [
            (
                r#"Review: {"outcome":"failure"} Final: {"outcome":"success"}"#,
                "2 distinct JSON candidates",
            ),
            (
                r#"{"result":{"outcome":"success"},"skill_result":{"outcome":"failure"}}"#,
                "ambiguous completion envelope fields",
            ),
            (
                r#"Review: "{\"outcome\":\"failure\"}" Final: {"outcome":"success"}"#,
                "2 distinct JSON candidates",
            ),
            (
                r#"Review: {"outcome":"success"} Earlier: "{\"outcome\":\"failure\"}""#,
                "2 distinct JSON candidates",
            ),
            (
                r#"Review: {"outcome":"success","checklist":[1]} Final: {"outcome":"success","checklist":[2]}"#,
                "2 distinct JSON candidates",
            ),
            (
                r#"Review: {"outcome":"success","evidence":[42]} Final: {"outcome":"success","evidence":["42"]}"#,
                "2 distinct JSON candidates",
            ),
        ] {
            let error = decode_result(raw, "i", "b", "quality", true)
                .unwrap_err()
                .to_string();
            assert!(error.contains(diagnostic), "{error}");
        }
    }

    #[test]
    fn provider_extensions_do_not_bypass_transport_bounds() {
        use crate::application::agent_io::structured_output::DecodeOptions;
        let options = DecodeOptions::new("Skill completion report");
        let oversized = json!({"outcome":"success", "extra":"x".repeat(options.max_bytes)});
        let mut nested = Value::Null;
        for _ in 0..=options.max_depth {
            nested = json!([nested]);
        }
        let deep = json!({"outcome":"success", "extra":nested});
        let deep_evidence = json!({"outcome":"success", "evidence":[nested]});
        let mut wrapped = json!({"outcome":"success", "checklist":[]});
        for _ in 0..options.max_layers {
            wrapped = json!({"result":wrapped});
        }
        assert!(decode_result(&wrapped.to_string(), "i", "b", "quality", true).is_ok());
        let wrapped = json!({"result":wrapped});
        let mut stringified = r#"{"outcome":"success","checklist":[]}"#.to_string();
        for _ in 0..=options.max_layers {
            stringified = serde_json::to_string(&stringified).unwrap();
        }
        for (output, diagnostic) in [
            (oversized.to_string(), "maximum payload size"),
            (deep.to_string(), "maximum JSON nesting depth"),
            (deep_evidence.to_string(), "maximum JSON nesting depth"),
            (
                wrapped.to_string(),
                "completion-envelope or stringification layers",
            ),
            (stringified, "completion-envelope or stringification layers"),
        ] {
            for transported in [output.clone(), format!("Review complete. {output} Done.")] {
                let error = decode_result(&transported, "i", "b", "quality", true)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(diagnostic), "{error}");
            }
        }
    }
}
