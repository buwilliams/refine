use serde_json::to_string_pretty;

use crate::model::goal::{ImplementationCriticism, ProposedImplementationPlan};
use crate::process::supervisor::errors::{RefineError, RefineResult};

const PLAN_SCHEMA: &str = r#"{"summary":"...","checklist":[{"id":"P1","description":"...","affected_behavior":["..."],"governance_rationale":"... or null","verification":["exact evidence"]}],"criticism_resolutions":[]}"#;
const CRITICISM_SCHEMA: &str = r#"{"summary":"...","findings":[{"id":"C1","material":true,"checklist_item_ids":["P1"],"description":"...","recommendation":"..."}]}"#;

pub(super) fn planning_prompt(spec: &str) -> String {
    format!(
        "{spec}\n\n# Current Workflow Phase: Plan\n\nInspect the real repository and the complete pinned scenario above. This phase is observational: do not edit files, create commits, change branches, or mutate repository state. Propose one actionable implementation plan. Use stable checklist IDs, name affected behavior and surfaces, explain Governance relevance, and name intended verification evidence. Complete this phase by putting JSON matching the following schema in the completion signal's planning_result field:\n{PLAN_SCHEMA}"
    )
}

pub(super) fn criticism_prompt(
    spec: &str,
    proposal: &ProposedImplementationPlan,
) -> RefineResult<String> {
    let proposal = to_string_pretty(proposal).map_err(encode_error)?;
    Ok(format!(
        "{spec}\n\n# Current Workflow Phase: Criticize\n\nYou are a fresh, independent critic. Inspect the same real repository and pinned scenario. This phase is observational: do not mutate repository state. Find material omissions, incorrect assumptions, cross-surface inconsistencies, failure or recovery gaps, and Governance conflicts. This is model judgment, not a deterministic checklist verdict.\n\n## Proposed Plan\n```json\n{proposal}\n```\n\nComplete this phase by putting JSON matching the following schema in the completion signal's planning_result field:\n{CRITICISM_SCHEMA}"
    ))
}

pub(super) fn revision_prompt(
    spec: &str,
    proposal: &ProposedImplementationPlan,
    criticism: &ImplementationCriticism,
) -> RefineResult<String> {
    let proposal = to_string_pretty(proposal).map_err(encode_error)?;
    let criticism = to_string_pretty(criticism).map_err(encode_error)?;
    Ok(format!(
        "{spec}\n\n# Current Workflow Phase: Revise\n\nYou are a fresh planning agent. Inspect the same real repository and pinned scenario. This phase is observational: do not mutate repository state. Produce the final plan and stable checklist. Resolve every material criticism or explain why it does not apply in criticism_resolutions.\n\n## Original Proposal\n```json\n{proposal}\n```\n\n## Independent Criticism\n```json\n{criticism}\n```\n\nComplete this phase by putting JSON matching the following schema in the completion signal's planning_result field:\n{PLAN_SCHEMA}"
    ))
}

pub(super) fn implementation_prompt(
    spec: &str,
    final_plan: &ProposedImplementationPlan,
) -> RefineResult<String> {
    let final_plan = to_string_pretty(final_plan).map_err(encode_error)?;
    Ok(format!(
        "{spec}\n\n# Governed Execution-Time Implementation Plan\n\nThe final Round request in the pinned specification remains authoritative. Use the accepted plan below as execution guidance, not approval. Implement autonomously, and preserve post-implementation Guidance, Governance, Quality, Ready Merge, and Review boundaries. Your completion signal must include implementation_evidence reporting completed, deviated, rejected, or blocked for every stable checklist ID plus exact verification evidence. Do not rewrite the accepted plan when execution differs; record the discrepancy.\n\n```json\n{final_plan}\n```"
    ))
}

fn encode_error(error: serde_json::Error) -> RefineError {
    RefineError::Serialization(format!(
        "failed to encode implementation planning prompt: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::goal::{ImplementationChecklistItem, ProposedImplementationPlan};

    #[test]
    fn implementation_prompt_keeps_round_authority_and_includes_final_checklist() {
        let prompt = implementation_prompt(
            "Latest Round request: authoritative request",
            &ProposedImplementationPlan {
                summary: "Final plan".to_string(),
                checklist: vec![ImplementationChecklistItem {
                    id: "P1".to_string(),
                    description: "Implement shared behavior".to_string(),
                    affected_behavior: vec!["CLI and browser".to_string()],
                    governance_rationale: None,
                    verification: vec!["cargo test --lib".to_string()],
                }],
                criticism_resolutions: Vec::new(),
            },
        )
        .unwrap();
        assert!(prompt.contains("Latest Round request: authoritative request"));
        assert!(prompt.contains("final Round request"));
        assert!(prompt.contains("\"id\": \"P1\""));
        assert!(prompt.contains("cargo test --lib"));
    }
}
