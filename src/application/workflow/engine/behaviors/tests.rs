use super::*;

#[cfg(unix)]
mod already_merged_quality_failure;
#[cfg(unix)]
mod quality_resume;

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
fn exhausted_quality_output_repair_keeps_a_distinct_workflow_failure_category() {
    let error = RefineError::StructuredOutput(
        crate::application::agent_io::structured_output::StructuredOutputError::transport(
            "Quality evaluation JSON",
            "contains invalid JSON: expected value at line 1 column 1",
        ),
    );

    assert_eq!(quality_failure_category(&error), "quality_output_contract");
}
