use crate::model::goal::{
    CriticismResolution, ImplementationChecklistItem, ProposedImplementationPlan,
};

/// A populated revise result serialized from the durable planning model.
pub fn revision_result_contract_json() -> String {
    serde_json::to_string(&ProposedImplementationPlan {
        summary: "one plain-language paragraph explaining what will change and why".to_string(),
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
    })
    .expect("typed revise planning contract must serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_contract_uses_the_canonical_typed_resolution_shape() {
        let encoded = revision_result_contract_json();
        let decoded: ProposedImplementationPlan = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.criticism_resolutions[0].criticism_id, "C1");
        assert!(encoded.contains(r#""criticism_id":"C1""#));
        assert!(encoded.contains(r#""resolution":"how the revised plan resolves"#));
    }
}
