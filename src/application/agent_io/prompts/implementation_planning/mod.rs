use crate::application::agent_io::structured_output::Contract;
use crate::model::goal::{
    ImplementationCriticism, ImplementationExecutionEvidence, ProposedImplementationPlan,
};

/// The plan-phase result contract: the shared example without revise-only
/// criticism resolutions.
pub fn plan_result_contract_json() -> String {
    let mut example = ProposedImplementationPlan::example();
    example.criticism_resolutions.clear();
    serde_json::to_string(&example).expect("typed plan contract must serialize")
}

/// The criticize-phase result contract with one populated material finding.
pub fn criticism_result_contract_json() -> String {
    ImplementationCriticism::contract_json()
}

/// A populated revise result serialized from the durable planning model.
pub fn revision_result_contract_json() -> String {
    ProposedImplementationPlan::contract_json()
}

/// The implement-phase execution evidence contract.
pub fn implementation_evidence_contract_json() -> String {
    ImplementationExecutionEvidence::contract_json()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_contracts_render_from_the_durable_planning_types() {
        let plan = plan_result_contract_json();
        assert!(plan.contains(r#""id":"P1""#));
        assert!(!plan.contains("criticism_resolutions"));
        assert!(!plan.contains("affected_behavior"));

        let criticism = criticism_result_contract_json();
        assert!(criticism.contains(r#""id":"C1""#));
        assert!(criticism.contains(r#""material":true"#));
        assert!(criticism.contains(r#""checklist_item_ids":["P1"]"#));

        let revision = revision_result_contract_json();
        assert!(revision.contains(r#""criticism_id":"C1""#));
        assert!(revision.contains(r#""resolution":"how the revised plan resolves"#));

        let evidence = implementation_evidence_contract_json();
        assert!(evidence.contains(r#""outcome":"completed""#));
        assert!(evidence.contains(r#""verification":["exact command and result"]"#));
    }
}
