use super::*;

#[test]
fn goal_agent_context_pins_governance_and_only_enabled_guidance() {
    let context = goal_agent_context(
        &json!({
            "product": "Refine",
            "constitution": "Preserve audit boundaries.",
            "rules": [{"text": "Stop at Review."}],
            "configured": true
        }),
        &json!({
            "guidance": [
                {"name": "Architecture", "enabled": true},
                {"name": "Retired", "enabled": false},
                {"name": "Default enabled"}
            ]
        }),
        &json!({
            "id": "GOAL1",
            "name": "Context contract",
            "priority": "high",
            "rounds": [
                {"reporter": "A", "prompt": "Earlier request"},
                {"reporter": "B", "prompt": "Current request"}
            ]
        }),
        1,
    )
    .unwrap();

    assert_eq!(context["version"], 1);
    assert_eq!(context["governance"]["product"], "Refine");
    assert_eq!(context["governance"]["configured"], true);
    assert_eq!(context["guidance_candidates"].as_array().unwrap().len(), 2);
    assert_eq!(context["goal"]["name"], "Context contract");
    assert_eq!(context["previous_rounds"][0]["prompt"], "Earlier request");
    assert_eq!(context["current_round"]["prompt"], "Current request");
    assert!(
        context["workflow_summary"]
            .as_str()
            .unwrap()
            .contains("human Review")
    );
}

#[test]
fn same_turn_guidance_decision_resolves_candidates_without_an_extra_provider() {
    let context = json!({
        "version": 1,
        "guidance_candidates": [
            {"name": "Architecture", "rule": "Architecture changes", "instructions": "Preserve boundaries."},
            {"name": "Mobile", "rule": "Mobile changes", "instructions": "Test touch behavior."}
        ]
    });

    let decision = guidance_decision(&context, Some(&[0])).unwrap();

    assert_eq!(decision["context_version"], 1);
    assert_eq!(decision["applied"][0]["name"], "Architecture");
    assert_eq!(decision["skipped"][0]["name"], "Mobile");
}

#[test]
fn guidance_candidates_require_an_explicit_valid_completion_selection() {
    let context = json!({
        "version": 1,
        "guidance_candidates": [
            {"name": "Architecture", "rule": "Architecture changes", "instructions": "Preserve boundaries."}
        ]
    });

    assert!(guidance_decision(&context, None).is_err());
    assert!(guidance_decision(&context, Some(&[0, 0])).is_err());
    assert!(guidance_decision(&context, Some(&[1])).is_err());
    assert!(guidance_decision(&context, Some(&[])).is_ok());
}
