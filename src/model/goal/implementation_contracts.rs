//! Structured-output contracts for governed implementation planning.
//!
//! The serde types in [`super::implementation_plan`] are the single source of
//! truth: prompts render their JSON examples from [`Contract::example`], and
//! agent output decodes back through the same types. Semantic rules that need
//! no external context live in [`Contract::validate`]; cross-artifact rules
//! (like resolution coverage of a specific criticism) stay with the workflow.

use serde_json::{Map, Value};

use crate::structured_output::{Contract, StructuredOutputError};

use super::implementation_plan::{
    ImplementationChecklistItem, ImplementationChecklistOutcome, ImplementationChecklistResult,
    ImplementationCriticism, ImplementationCriticismFinding, ImplementationExecutionEvidence,
    CriticismResolution, ProposedImplementationPlan,
};

pub const MAX_CRITICISM_FINDINGS: usize = 3;
pub const MAX_SUMMARY_CHARS: usize = 20_000;
pub const MAX_ITEM_CHARS: usize = 28_000;

impl Contract for ProposedImplementationPlan {
    const LABEL: &'static str = "implementation plan JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["planning_result", "result"];

    fn example() -> Self {
        ProposedImplementationPlan {
            summary: "one single-line plain-language summary of what will change and why"
                .to_string(),
            checklist: vec![ImplementationChecklistItem {
                id: "P1".to_string(),
                description: "one implementation step that clearly advances the plan".to_string(),
                affected_behavior: Vec::new(),
                governance_rationale: None,
                verification: Vec::new(),
            }],
            criticism_resolutions: vec![CriticismResolution {
                criticism_id: "C1".to_string(),
                resolution: "how the revised plan resolves this material finding".to_string(),
            }],
        }
    }

    fn normalize(value: &mut Value) -> Result<(), StructuredOutputError> {
        normalize_criticism_resolution_ids(value)
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        let invalid = |detail: String| StructuredOutputError::validation(Self::LABEL, detail);
        if self.summary.trim().is_empty() || self.checklist.is_empty() {
            return Err(invalid(
                "implementation plan requires a summary and at least one checklist item"
                    .to_string(),
            ));
        }
        validate_compact_text(
            Self::LABEL,
            "implementation plan summary",
            &self.summary,
            MAX_SUMMARY_CHARS,
        )?;
        let mut ids = std::collections::BTreeSet::new();
        for item in &self.checklist {
            if item.id.trim().is_empty() || item.description.trim().is_empty() {
                return Err(invalid(
                    "implementation checklist items require stable ids and descriptions"
                        .to_string(),
                ));
            }
            validate_compact_text(
                Self::LABEL,
                "implementation checklist item description",
                &item.description,
                MAX_ITEM_CHARS,
            )?;
            if !item.affected_behavior.is_empty()
                || item.governance_rationale.is_some()
                || !item.verification.is_empty()
            {
                return Err(invalid(
                    "implementation checklist items must be compact id/description pairs; affected behavior, Governance rationale, and verification belong in execution evidence"
                        .to_string(),
                ));
            }
            if !ids.insert(item.id.as_str()) {
                return Err(invalid(format!(
                    "implementation checklist repeats id {}",
                    item.id
                )));
            }
        }
        let mut resolution_ids = std::collections::BTreeSet::new();
        for resolution in &self.criticism_resolutions {
            if resolution.criticism_id.trim().is_empty() || resolution.resolution.trim().is_empty()
            {
                return Err(invalid(
                    "implementation criticism resolutions require an id and concise resolution"
                        .to_string(),
                ));
            }
            validate_compact_text(
                Self::LABEL,
                "implementation criticism resolution",
                &resolution.resolution,
                MAX_ITEM_CHARS,
            )?;
            if !resolution_ids.insert(resolution.criticism_id.as_str()) {
                return Err(invalid(format!(
                    "implementation plan repeats criticism resolution {}",
                    resolution.criticism_id
                )));
            }
        }
        Ok(())
    }
}

impl Contract for ImplementationCriticism {
    const LABEL: &'static str = "implementation criticism JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["planning_result", "result"];

    fn example() -> Self {
        ImplementationCriticism {
            summary: "one short sentence".to_string(),
            findings: vec![ImplementationCriticismFinding {
                id: "C1".to_string(),
                material: true,
                checklist_item_ids: vec!["P1".to_string()],
                description: "one material omission".to_string(),
                recommendation: "one concise correction".to_string(),
            }],
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        let invalid = |detail: String| StructuredOutputError::validation(Self::LABEL, detail);
        validate_compact_text(
            Self::LABEL,
            "implementation criticism summary",
            &self.summary,
            MAX_SUMMARY_CHARS,
        )?;
        if self.findings.len() > MAX_CRITICISM_FINDINGS {
            return Err(invalid(format!(
                "implementation criticism must contain at most {MAX_CRITICISM_FINDINGS} material findings"
            )));
        }
        let mut ids = std::collections::BTreeSet::new();
        for finding in &self.findings {
            if finding.id.trim().is_empty()
                || finding.description.trim().is_empty()
                || finding.recommendation.trim().is_empty()
            {
                return Err(invalid(
                    "implementation criticism findings require an id, description, and recommendation"
                        .to_string(),
                ));
            }
            validate_compact_text(
                Self::LABEL,
                "implementation criticism finding description",
                &finding.description,
                MAX_ITEM_CHARS,
            )?;
            validate_compact_text(
                Self::LABEL,
                "implementation criticism finding recommendation",
                &finding.recommendation,
                MAX_ITEM_CHARS,
            )?;
            if !finding.material {
                return Err(invalid(
                    "implementation criticism may include only material findings".to_string(),
                ));
            }
            if !ids.insert(finding.id.as_str()) {
                return Err(invalid(format!(
                    "implementation criticism repeats finding id {}",
                    finding.id
                )));
            }
        }
        Ok(())
    }
}

impl Contract for ImplementationExecutionEvidence {
    const LABEL: &'static str = "implementation evidence JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["implementation_evidence"];

    fn example() -> Self {
        ImplementationExecutionEvidence {
            checklist: vec![ImplementationChecklistResult {
                id: "P1".to_string(),
                outcome: ImplementationChecklistOutcome::Completed,
                evidence: "what happened".to_string(),
            }],
            verification: vec!["exact command and result".to_string()],
        }
    }
}

fn validate_compact_text(
    contract_label: &str,
    field_label: &str,
    value: &str,
    max_chars: usize,
) -> Result<(), StructuredOutputError> {
    let invalid = |detail: String| StructuredOutputError::validation(contract_label, detail);
    if value.trim().is_empty() {
        return Err(invalid(format!("{field_label} must not be empty")));
    }
    if value.contains(['\n', '\r']) {
        return Err(invalid(format!("{field_label} must be one line")));
    }
    let length = value.chars().count();
    if length > max_chars {
        return Err(invalid(format!(
            "{field_label} must be concise (at most {max_chars} characters, observed {length})"
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
            ProposedImplementationPlan::LABEL,
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
    use crate::structured_output::assert_contract_roundtrip;

    #[test]
    fn planning_contract_examples_roundtrip_through_their_own_decoders() {
        assert_contract_roundtrip::<ProposedImplementationPlan>();
        assert_contract_roundtrip::<ImplementationCriticism>();
        assert_contract_roundtrip::<ImplementationExecutionEvidence>();
    }
}
