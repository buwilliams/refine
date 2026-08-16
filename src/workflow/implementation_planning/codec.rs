use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::model::goal::{ImplementationCriticism, ProposedImplementationPlan};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::structured_output::{DecodeOptions, StructuredOutputError, decode_structured};

const MAX_CRITICISM_FINDINGS: usize = 3;
const MAX_SUMMARY_CHARS: usize = 20_000;
const MAX_ITEM_CHARS: usize = 28_000;

const COMPLETION_ENVELOPE_FIELDS: &[&str] = &["planning_result", "result"];

pub(super) fn decode_plan(output: &str) -> RefineResult<ProposedImplementationPlan> {
    let plan: ProposedImplementationPlan = decode_structured(
        output,
        &DecodeOptions::with_envelopes("implementation plan JSON", COMPLETION_ENVELOPE_FIELDS),
        normalize_criticism_resolution_ids,
    )?;
    validate_plan(&plan)?;
    Ok(plan)
}

pub(super) fn decode_criticism(output: &str) -> RefineResult<ImplementationCriticism> {
    let criticism: ImplementationCriticism = decode_structured(
        output,
        &DecodeOptions::with_envelopes(
            "implementation criticism JSON",
            COMPLETION_ENVELOPE_FIELDS,
        ),
        |_| Ok(()),
    )?;
    validate_compact_text(
        "implementation criticism summary",
        &criticism.summary,
        MAX_SUMMARY_CHARS,
    )?;
    if criticism.findings.len() > MAX_CRITICISM_FINDINGS {
        return Err(RefineError::Serialization(format!(
            "implementation criticism must contain at most {MAX_CRITICISM_FINDINGS} material findings"
        )));
    }
    let mut ids = BTreeSet::new();
    for finding in &criticism.findings {
        if finding.id.trim().is_empty()
            || finding.description.trim().is_empty()
            || finding.recommendation.trim().is_empty()
        {
            return Err(RefineError::Serialization(
                "implementation criticism findings require an id, description, and recommendation"
                    .to_string(),
            ));
        }
        validate_compact_text(
            "implementation criticism finding description",
            &finding.description,
            MAX_ITEM_CHARS,
        )?;
        validate_compact_text(
            "implementation criticism finding recommendation",
            &finding.recommendation,
            MAX_ITEM_CHARS,
        )?;
        if !finding.material {
            return Err(RefineError::Serialization(
                "implementation criticism may include only material findings".to_string(),
            ));
        }
        if !ids.insert(finding.id.as_str()) {
            return Err(RefineError::Serialization(format!(
                "implementation criticism repeats finding id {}",
                finding.id
            )));
        }
    }
    Ok(criticism)
}

pub(super) fn validate_revised_plan(
    plan: &ProposedImplementationPlan,
    criticism: &ImplementationCriticism,
) -> RefineResult<()> {
    validate_plan(plan)?;
    let resolved = plan
        .criticism_resolutions
        .iter()
        .map(|resolution| resolution.criticism_id.as_str())
        .collect::<BTreeSet<_>>();
    let material = criticism
        .findings
        .iter()
        .filter(|finding| finding.material)
        .map(|finding| finding.id.as_str())
        .collect::<BTreeSet<_>>();
    let unrelated = resolved.difference(&material).copied().collect::<Vec<_>>();
    if !unrelated.is_empty() {
        return Err(RefineError::Serialization(format!(
            "revised implementation plan resolved unknown or non-material criticism: {}",
            unrelated.join(", ")
        )));
    }
    let unresolved = criticism
        .findings
        .iter()
        .filter(|finding| finding.material && !resolved.contains(finding.id.as_str()))
        .map(|finding| finding.id.clone())
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Err(RefineError::Serialization(format!(
            "revised implementation plan did not resolve material criticism: {}",
            unresolved.join(", ")
        )));
    }
    Ok(())
}

fn validate_plan(plan: &ProposedImplementationPlan) -> RefineResult<()> {
    if plan.summary.trim().is_empty() || plan.checklist.is_empty() {
        return Err(RefineError::Serialization(
            "implementation plan requires a summary and at least one checklist item".to_string(),
        ));
    }
    validate_compact_text(
        "implementation plan summary",
        &plan.summary,
        MAX_SUMMARY_CHARS,
    )?;
    let mut ids = BTreeSet::new();
    for item in &plan.checklist {
        if item.id.trim().is_empty() || item.description.trim().is_empty() {
            return Err(RefineError::Serialization(
                "implementation checklist items require stable ids and descriptions".to_string(),
            ));
        }
        validate_compact_text(
            "implementation checklist item description",
            &item.description,
            MAX_ITEM_CHARS,
        )?;
        if !item.affected_behavior.is_empty()
            || item.governance_rationale.is_some()
            || !item.verification.is_empty()
        {
            return Err(RefineError::Serialization(
                "implementation checklist items must be compact id/description pairs; affected behavior, Governance rationale, and verification belong in execution evidence"
                    .to_string(),
            ));
        }
        if !ids.insert(item.id.as_str()) {
            return Err(RefineError::Serialization(format!(
                "implementation checklist repeats id {}",
                item.id
            )));
        }
    }
    let mut resolution_ids = BTreeSet::new();
    for resolution in &plan.criticism_resolutions {
        if resolution.criticism_id.trim().is_empty() || resolution.resolution.trim().is_empty() {
            return Err(RefineError::Serialization(
                "implementation criticism resolutions require an id and concise resolution"
                    .to_string(),
            ));
        }
        validate_compact_text(
            "implementation criticism resolution",
            &resolution.resolution,
            MAX_ITEM_CHARS,
        )?;
        if !resolution_ids.insert(resolution.criticism_id.as_str()) {
            return Err(RefineError::Serialization(format!(
                "implementation plan repeats criticism resolution {}",
                resolution.criticism_id
            )));
        }
    }
    Ok(())
}

fn validate_compact_text(label: &str, value: &str, max_chars: usize) -> RefineResult<()> {
    if value.trim().is_empty() {
        return Err(RefineError::Serialization(format!(
            "{label} must not be empty"
        )));
    }
    if value.contains(['\n', '\r']) {
        return Err(RefineError::Serialization(format!(
            "{label} must be one line"
        )));
    }
    let length = value.chars().count();
    if length > max_chars {
        return Err(RefineError::Serialization(format!(
            "{label} must be concise (at most {max_chars} characters, observed {length})"
        )));
    }
    Ok(())
}

fn normalize_criticism_resolution_ids(value: &mut Value) -> Result<(), StructuredOutputError> {
    let Some(resolutions) = value
        .as_object_mut()
        .and_then(|plan| plan.get_mut("criticism_resolutions"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for (index, resolution) in resolutions.iter_mut().enumerate() {
        let Some(resolution) = resolution.as_object_mut() else {
            continue;
        };
        normalize_resolution_id(resolution, index)?;
    }
    Ok(())
}

fn normalize_resolution_id(
    resolution: &mut Map<String, Value>,
    index: usize,
) -> Result<(), StructuredOutputError> {
    const ALIASES: [&str; 3] = ["criticismId", "finding_id", "id"];
    let aliases = ALIASES
        .into_iter()
        .filter(|alias| resolution.contains_key(*alias))
        .collect::<Vec<_>>();
    if resolution.contains_key("criticism_id") && !aliases.is_empty() || aliases.len() > 1 {
        let mut fields = Vec::from(["criticism_id"]);
        fields.retain(|field| resolution.contains_key(*field));
        fields.extend(aliases);
        return Err(StructuredOutputError::schema(
            "implementation plan JSON",
            Some(format!("criticism_resolutions[{index}]")),
            format!("has ambiguous identifier fields: {}", fields.join(", ")),
        ));
    }
    if let Some(alias) = aliases.first() {
        let id = resolution
            .remove(*alias)
            .expect("present resolution id alias");
        resolution.insert("criticism_id".to_string(), id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_decoding_accepts_completion_signal_wrappers_and_stringified_results() {
        let wrapped = decode_plan(
            r#"{"state":"completed","message":"done","planning_result":{"summary":"Change the boundary","checklist":[{"id":"P1","description":"Implement the required behavior."}]}}"#,
        )
        .unwrap();
        assert_eq!(wrapped.checklist[0].id, "P1");

        let stringified = decode_plan(
            r#"{"planning_result":"{\"summary\":\"Change the boundary\",\"checklist\":[{\"id\":\"P1\",\"description\":\"Implement the required behavior.\"}]}"}"#,
        )
        .unwrap();
        assert_eq!(stringified, wrapped);
    }

    #[test]
    fn plans_require_compact_one_line_items_but_allow_as_many_as_needed() {
        let oversized = format!(
            r#"{{"summary":"Do it","checklist":[{{"id":"P1","description":"{}"}}]}}"#,
            "x".repeat(MAX_ITEM_CHARS + 1)
        );
        assert!(decode_plan(&oversized).is_err());

        let many = r#"{"summary":"Do it","checklist":[{"id":"P1","description":"One"},{"id":"P2","description":"Two"},{"id":"P3","description":"Three"},{"id":"P4","description":"Four"},{"id":"P5","description":"Five"}]}"#;
        assert_eq!(decode_plan(many).unwrap().checklist.len(), 5);

        let multiline = r#"{"summary":"Do it","checklist":[{"id":"P1","description":"One\nTwo"}]}"#;
        assert!(decode_plan(multiline).is_err());

        let verbose_shape = r#"{"summary":"Do it","checklist":[{"id":"P1","description":"One","verification":["cargo test"]}]}"#;
        assert!(decode_plan(verbose_shape).is_err());
    }

    #[test]
    fn plan_summaries_allow_paragraphs_beyond_the_former_600_char_limit() {
        let formerly_oversized = format!(
            r#"{{"summary":"{}","checklist":[{{"id":"P1","description":"Change it"}}]}}"#,
            "x".repeat(805)
        );
        assert!(decode_plan(&formerly_oversized).is_ok());

        let oversized = format!(
            r#"{{"summary":"{}","checklist":[{{"id":"P1","description":"Change it"}}]}}"#,
            "x".repeat(MAX_SUMMARY_CHARS + 1)
        );
        assert!(decode_plan(&oversized).is_err());
    }

    #[test]
    fn criticism_details_are_guided_toward_conciseness_without_a_tiny_limit() {
        let formerly_oversized = serde_json::json!({
            "summary": "Material gap",
            "findings": [{
                "id": "C1",
                "material": true,
                "description": "x".repeat(307),
                "recommendation": "Correct the material gap."
            }]
        });
        assert!(decode_criticism(&formerly_oversized.to_string()).is_ok());

        let oversized = serde_json::json!({
            "summary": "Material gap",
            "findings": [{
                "id": "C1",
                "material": true,
                "description": "x".repeat(MAX_ITEM_CHARS + 1),
                "recommendation": "Correct the material gap."
            }]
        });
        assert!(decode_criticism(&oversized.to_string()).is_err());
    }

    #[test]
    fn revised_plan_must_resolve_every_material_criticism() {
        let plan = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}]}"#,
        )
        .unwrap();
        let criticism = decode_criticism(
            r#"{"summary":"Gap","findings":[{"id":"C1","material":true,"description":"Missing failure path","recommendation":"Add it"}]}"#,
        )
        .unwrap();
        assert!(validate_revised_plan(&plan, &criticism).is_err());
    }

    fn material_criticism() -> ImplementationCriticism {
        decode_criticism(
            r#"{"summary":"Gap","findings":[{"id":"C1","material":true,"description":"Missing failure path","recommendation":"Add it"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn revised_plans_normalize_only_supported_resolution_identifier_spellings() {
        for field in ["criticism_id", "criticismId", "finding_id", "id"] {
            let output = format!(
                r#"{{"state":"completed","planning_result":{{"summary":"Do it","checklist":[{{"id":"P1","description":"Change it"}}],"criticism_resolutions":[{{"{field}":"C1","resolution":"Added the failure path"}}]}}}}"#
            );
            let plan = decode_plan(&output).unwrap();
            assert_eq!(plan.criticism_resolutions[0].criticism_id, "C1");
            validate_revised_plan(&plan, &material_criticism()).unwrap();
        }
    }

    #[test]
    fn revised_plans_reject_malformed_ambiguous_duplicate_unknown_and_incomplete_resolutions() {
        let malformed = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(malformed.contains("invalid JSON"));

        let ambiguous = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1","id":"C1","resolution":"Added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(ambiguous.contains("ambiguous identifier fields"));

        let unsupported_alias = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticismID":"C1","resolution":"Added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(unsupported_alias.contains("unknown field `criticismID`"));

        let duplicate = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1","resolution":"Added it"},{"criticismId":"C1","resolution":"Also added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(duplicate.contains("repeats criticism resolution C1"));

        let unknown = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C2","resolution":"Added it"}]}"#,
        )
        .unwrap();
        assert!(
            validate_revised_plan(&unknown, &material_criticism())
                .unwrap_err()
                .to_string()
                .contains("unknown or non-material criticism: C2")
        );

        let incomplete = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(incomplete.contains("field `criticism_resolutions[0]`"));
        assert!(incomplete.contains("missing field `resolution`"));

        let missing_id = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"resolution":"Added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(missing_id.contains("field `criticism_resolutions[0]`"));
        assert!(missing_id.contains("missing field `criticism_id`"));

        let structurally_different = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1","resolution":"Added it","details":"extra shape"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(structurally_different.contains("unknown field `details`"));

        let over_nested = format!("{}0{}", "[".repeat(40), "]".repeat(40));
        assert!(
            decode_plan(&over_nested)
                .unwrap_err()
                .to_string()
                .contains("maximum JSON nesting depth")
        );
    }
}
