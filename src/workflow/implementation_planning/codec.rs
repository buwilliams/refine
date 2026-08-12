use std::collections::BTreeSet;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::model::goal::{ImplementationCriticism, ProposedImplementationPlan};
use crate::process::supervisor::errors::{RefineError, RefineResult};

const MAX_CRITICISM_FINDINGS: usize = 3;
const MAX_SUMMARY_CHARS: usize = 600;
const MAX_ITEM_CHARS: usize = 280;

pub(super) fn decode_plan(output: &str) -> RefineResult<ProposedImplementationPlan> {
    let plan: ProposedImplementationPlan = decode_json(output, "implementation plan")?;
    validate_plan(&plan)?;
    Ok(plan)
}

pub(super) fn decode_criticism(output: &str) -> RefineResult<ImplementationCriticism> {
    let criticism: ImplementationCriticism = decode_json(output, "implementation criticism")?;
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

fn decode_json<T: DeserializeOwned>(output: &str, label: &str) -> RefineResult<T> {
    let output = output.trim();
    let candidates = [
        Some(output),
        output.find("```json").and_then(|start| {
            output[start + 7..]
                .find("```")
                .map(|end| &output[start + 7..start + 7 + end])
        }),
        output
            .find('{')
            .zip(output.rfind('}'))
            .filter(|(start, end)| start <= end)
            .map(|(start, end)| &output[start..=end]),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Ok(value) = serde_json::from_str::<Value>(candidate.trim())
            && let Some(decoded) = decode_value(value)
        {
            return Ok(decoded);
        }
    }
    Err(RefineError::Serialization(format!(
        "agent returned no valid structured {label} JSON"
    )))
}

fn decode_value<T: DeserializeOwned>(value: Value) -> Option<T> {
    if let Ok(decoded) = serde_json::from_value(value.clone()) {
        return Some(decoded);
    }
    match value {
        Value::Object(mut object) => object
            .remove("planning_result")
            .or_else(|| object.remove("result"))
            .and_then(decode_value),
        Value::String(encoded) => serde_json::from_str::<Value>(&encoded)
            .ok()
            .and_then(decode_value),
        _ => None,
    }
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
}
