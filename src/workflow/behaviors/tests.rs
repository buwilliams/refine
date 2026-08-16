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
fn same_turn_guidance_decision_resolves_candidates_without_an_extra_provider() {
    let context = json!({
        "version": 1,
        "guidance_candidates": [
            {"name": "Architecture", "rule": "Architecture changes", "instructions": "Preserve boundaries."},
            {"name": "Mobile", "rule": "Mobile changes", "instructions": "Test touch behavior."}
        ]
    });

    let decision = guidance_decision(&context, Some(&[0]), false).unwrap();

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

    assert!(guidance_decision(&context, None, false).is_err());
    assert!(guidance_decision(&context, Some(&[0, 0]), false).is_err());
    assert!(guidance_decision(&context, Some(&[1]), false).is_err());
    assert!(guidance_decision(&context, Some(&[]), false).is_ok());
}

#[test]
fn code_file_guidance_cannot_be_skipped_when_code_changed() {
    let context = json!({
        "version": 1,
        "guidance_candidates": [
            {
                "name": "Cohesive code files",
                "rule": CODE_FILE_GUIDANCE_RULE,
                "instructions": "Keep each code file focused."
            },
            {
                "name": "Mobile",
                "rule": "Apply to mobile changes.",
                "instructions": "Test touch behavior."
            }
        ]
    });

    let error = guidance_decision(&context, Some(&[]), true).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires it for changed code files")
    );

    let decision = guidance_decision(&context, Some(&[0]), true).unwrap();
    assert_eq!(decision["applied"][0]["name"], "Cohesive code files");
    assert_eq!(decision["skipped"][0]["name"], "Mobile");
    assert_eq!(decision["code_files_changed"], true);
}

#[test]
fn code_file_guidance_may_be_explicitly_skipped_for_no_code_change() {
    let context = json!({
        "version": 1,
        "guidance_candidates": [{
            "name": "Cohesive code files",
            "rule": CODE_FILE_GUIDANCE_RULE,
            "instructions": "Keep each code file focused."
        }]
    });

    let decision = guidance_decision(&context, Some(&[]), false).unwrap();
    assert_eq!(decision["skipped"][0]["name"], "Cohesive code files");
    assert_eq!(decision["code_files_changed"], false);
}

#[test]
fn code_path_detection_fails_safe_for_unlisted_languages() {
    assert!(is_code_path("src/worker.exs"));
    assert!(is_code_path("Makefile"));
    assert!(is_code_path("config/toolchain.toml"));
    assert!(!is_code_path("docs/architecture.md"));
    assert!(!is_code_path("website/diagram.png"));
}

#[test]
fn exhausted_quality_output_repair_keeps_a_distinct_workflow_failure_category() {
    let error =
        RefineError::StructuredOutput(crate::structured_output::StructuredOutputError::transport(
            "Quality evaluation JSON",
            "contains invalid JSON: expected value at line 1 column 1",
        ));

    assert_eq!(quality_failure_category(&error), "quality_output_contract");
}
