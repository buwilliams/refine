use serde_json::to_string_pretty;

use crate::model::goal::{ImplementationCriticism, ProposedImplementationPlan};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::implementation_planning::{
    criticism_result_contract_json, plan_result_contract_json, revision_result_contract_json,
};
use crate::prompts::{PromptTemplate, render};

pub(super) fn planning_prompt(spec: &str) -> String {
    let plan_contract = plan_result_contract_json();
    render(
        PromptTemplate::ImplementationPlanningPlan,
        &[("spec", spec), ("plan_contract", &plan_contract)],
    )
}

pub(super) fn criticism_prompt(
    spec: &str,
    proposal: &ProposedImplementationPlan,
) -> RefineResult<String> {
    let proposal = to_string_pretty(proposal).map_err(encode_error)?;
    let criticism_contract = criticism_result_contract_json();
    Ok(render(
        PromptTemplate::ImplementationPlanningCriticize,
        &[
            ("spec", spec),
            ("proposal", &proposal),
            ("criticism_contract", &criticism_contract),
        ],
    ))
}

pub(super) fn revision_prompt(
    spec: &str,
    proposal: &ProposedImplementationPlan,
    criticism: &ImplementationCriticism,
) -> RefineResult<String> {
    let proposal = to_string_pretty(proposal).map_err(encode_error)?;
    let criticism = to_string_pretty(criticism).map_err(encode_error)?;
    let revision_contract = revision_result_contract_json();
    Ok(render(
        PromptTemplate::ImplementationPlanningRevise,
        &[
            ("spec", spec),
            ("proposal", &proposal),
            ("criticism", &criticism),
            ("revision_contract", &revision_contract),
        ],
    ))
}

pub(super) fn implementation_prompt(
    spec: &str,
    final_plan: &ProposedImplementationPlan,
) -> RefineResult<String> {
    let final_plan = to_string_pretty(final_plan).map_err(encode_error)?;
    Ok(render(
        PromptTemplate::ImplementationPlanningImplement,
        &[("spec", spec), ("final_plan", &final_plan)],
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
    use crate::model::goal::{
        ImplementationChecklistItem, ImplementationCriticismFinding, ProposedImplementationPlan,
    };

    fn proposed_plan() -> ProposedImplementationPlan {
        ProposedImplementationPlan {
            summary: "Final plan".to_string(),
            checklist: vec![ImplementationChecklistItem {
                id: "P1".to_string(),
                description: "Implement shared behavior".to_string(),
                affected_behavior: Vec::new(),
                governance_rationale: None,
                verification: Vec::new(),
            }],
            criticism_resolutions: Vec::new(),
        }
    }

    #[test]
    fn observational_phase_templates_render_pinned_context_and_artifacts() {
        let spec = "Latest Round request: authoritative request";
        let proposal = proposed_plan();
        let criticism = ImplementationCriticism {
            summary: "One material omission".to_string(),
            findings: vec![ImplementationCriticismFinding {
                id: "C1".to_string(),
                material: true,
                checklist_item_ids: vec!["P1".to_string()],
                description: "Missing recovery coverage".to_string(),
                recommendation: "Add a recovery test".to_string(),
            }],
        };

        let plan = planning_prompt(spec);
        let criticize = criticism_prompt(spec, &proposal).unwrap();
        let revise = revision_prompt(spec, &proposal, &criticism).unwrap();

        for prompt in [&plan, &criticize, &revise] {
            assert!(prompt.contains(spec));
            assert!(!prompt.contains("{{"));
        }
        assert!(plan.contains("# Current Workflow Phase: Plan"));
        assert!(plan.contains("Checklist length is unrestricted"));
        assert!(!plan.contains("affected_behavior"));
        assert!(!plan.contains("governance_rationale"));
        assert!(criticize.contains("\"id\": \"P1\""));
        assert!(revise.contains("Missing recovery coverage"));
        assert!(revise.contains(r#""criticism_id":"C1""#));
        assert!(revise.contains(r#""resolution":"how the revised plan resolves"#));
    }

    #[test]
    fn implementation_prompt_keeps_round_authority_and_includes_final_checklist() {
        let prompt = implementation_prompt(
            "Latest Round request: authoritative request",
            &proposed_plan(),
        )
        .unwrap();
        assert!(prompt.contains("Latest Round request: authoritative request"));
        assert!(prompt.contains("final Round request"));
        assert!(prompt.contains("\"id\": \"P1\""));
        assert!(prompt.contains("Implement shared behavior"));
        assert!(!prompt.contains("affected_behavior"));
        assert!(!prompt.contains("\"verification\""));
    }
}
