use super::*;
use serde_json::json;

#[test]
fn goal_agent_spec_exposes_only_guidance_completion_indexes() {
    let prompt = goal_agent_prompt(
        "GOAL-GUIDANCE",
        &json!({
            "version": 1,
            "workflow_summary": "Implement through the governed workflow.",
            "governance": {
                "product": "Refine",
                "constitution": "Preserve workflow boundaries.",
                "rules": [],
                "configured": true
            },
            "guidance_candidates": [{
                "id": "guidance-1",
                "name": "Intent and architecture",
                "rule": "Apply to product behavior changes.",
                "instructions": "Preserve existing architecture.",
                "enabled": true
            }],
            "goal": {
                "id": "GOAL-GUIDANCE",
                "name": "Disambiguate Guidance completion identifiers"
            },
            "previous_rounds": [],
            "current_round": {
                "round": 1,
                "prompt": "Use the applicable Guidance."
            }
        }),
    )
    .unwrap();

    assert!(prompt.contains("### Enabled Guidance Candidates"));
    assert!(prompt.contains("#### 0. Intent and architecture"));
    assert!(prompt.contains("- **Completion Index:** 0"));
    assert!(prompt.contains("- **Applies When:** Apply to product behavior changes."));
    assert!(prompt.contains("- **Instructions:** Preserve existing architecture."));
    assert!(!prompt.contains("guidance-1"));
    assert!(!prompt.contains("**Id:**"));
}
