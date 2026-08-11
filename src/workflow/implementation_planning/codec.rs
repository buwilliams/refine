use std::collections::BTreeSet;

use serde::de::DeserializeOwned;

use crate::model::goal::{ImplementationCriticism, ProposedImplementationPlan};
use crate::process::supervisor::errors::{RefineError, RefineResult};

pub(super) fn decode_plan(output: &str) -> RefineResult<ProposedImplementationPlan> {
    let plan: ProposedImplementationPlan = decode_json(output, "implementation plan")?;
    validate_plan(&plan)?;
    Ok(plan)
}

pub(super) fn decode_criticism(output: &str) -> RefineResult<ImplementationCriticism> {
    let criticism: ImplementationCriticism = decode_json(output, "implementation criticism")?;
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
    let mut ids = BTreeSet::new();
    for item in &plan.checklist {
        if item.id.trim().is_empty() || item.description.trim().is_empty() {
            return Err(RefineError::Serialization(
                "implementation checklist items require stable ids and descriptions".to_string(),
            ));
        }
        if !ids.insert(item.id.as_str()) {
            return Err(RefineError::Serialization(format!(
                "implementation checklist repeats id {}",
                item.id
            )));
        }
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
        if let Ok(value) = serde_json::from_str(candidate.trim()) {
            return Ok(value);
        }
    }
    Err(RefineError::Serialization(format!(
        "agent returned no valid structured {label} JSON"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

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
