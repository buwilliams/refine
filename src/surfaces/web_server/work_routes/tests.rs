use super::*;
use crate::tools::product::imports::{ImportExtractionResult, PlanFeatureDestination};

fn plan_goal_extraction_result(draft_count: usize) -> ImportExtractionResult {
    ImportExtractionResult {
        drafts: (0..draft_count)
            .map(|index| ImportDraft {
                name: format!("Goal {}", index + 1),
                prompt: format!("Implement goal {}.", index + 1),
                reporter: String::new(),
                assignee: None,
                priority: "low".to_string(),
                duplicate_decision: String::new(),
                dependency_names: Vec::new(),
            })
            .collect(),
        feature_destination: Some(PlanFeatureDestination {
            name: "Should be discarded".to_string(),
            description: "Plan Goal creates no Feature.".to_string(),
        }),
    }
}

#[test]
fn plan_goal_extraction_requires_one_goal_and_discards_feature_destination() {
    let result =
        validate_import_extraction_result(plan_goal_extraction_result(1), "plan_goal").unwrap();
    assert_eq!(result.drafts.len(), 1);
    assert_eq!(result.feature_destination, None);

    let error =
        validate_import_extraction_result(plan_goal_extraction_result(2), "plan_goal").unwrap_err();
    assert_eq!(
        error.to_string(),
        "Plan Goal extraction must return exactly one Goal draft"
    );
}

#[test]
fn plan_import_result_sanitizes_feature_metadata_and_reads_feature_goals() {
    let output = json!({
        "feature": {
            "name": "Personal Budget App — Product Spec",
            "description": "created by Plan Mode",
            "goals": [
                {
                    "name": "Track spending by category",
                    "prompt": "Let users assign each transaction to a budget category.",
                    "priority": "medium"
                },
                {
                    "name": "Monthly budget overview",
                    "prompt": "Let users compare month-to-date spending against budget limits.",
                    "priority": "high"
                }
            ]
        }
    })
    .to_string();

    let result = parse_provider_import_result(&output, Some("Product")).unwrap();
    let feature = result.feature_destination.unwrap();
    assert_eq!(feature.name, "Personal Budget App");
    assert_eq!(feature.description, "");
    assert_eq!(result.drafts.len(), 2);
    assert_eq!(result.drafts[0].name, "Track spending by category");
    assert_eq!(
        result.drafts[0].prompt,
        "Let users assign each transaction to a budget category."
    );
    assert_eq!(result.drafts[0].reporter, "Product");
    assert_eq!(result.drafts[1].priority, "high");
}

#[test]
fn plan_import_result_reads_embedded_pretty_json_before_text_fallback() {
    let output = r#"Provider notes before JSON:
{
  "feature": {
    "name": "Smoke AI Plan Feature",
    "description": "A deterministic product capability planned by the Smoke AI fixture.",
    "goals": [
      {
        "name": "Smoke AI plan goal one",
        "prompt": "smoke-ai plan prompt one",
        "priority": "low"
      }
    ]
  }
}
Provider notes after JSON."#;

    let result = parse_provider_import_result(output, Some("Product")).unwrap();
    let feature = result.feature_destination.unwrap();
    assert_eq!(feature.name, "Smoke AI Plan Feature");
    assert_eq!(result.drafts.len(), 1);
    assert_eq!(result.drafts[0].prompt, "smoke-ai plan prompt one");
}

#[test]
fn plan_import_result_merges_feature_behavior_and_implementation_goal_arrays() {
    let output = json!({
            "feature": {
                "name": "Budget Alerts",
                "description": "Alert users when spending nears limits.",
                "goals": [
                    {
                        "name": "Budget threshold alert",
                        "prompt": "Alert users before a category exceeds its monthly budget.",
                        "priority": "high"
                    }
                ],
                "implementation_goals": [
                    {
                        "name": "Persist alert preferences",
                        "prompt": "Add a refine model that persists threshold preferences and exposes them through the budget settings API.",
                        "priority": "medium"
                    }
                ],
                "technical_goals": [
                    {
                        "name": "Verify alert trigger coverage",
                        "prompt": "Add automated tests for below-threshold, threshold-crossing, and disabled-alert cases.",
                        "priority": "medium"
                    }
                ]
            }
        })
        .to_string();

    let result = parse_provider_import_result(&output, Some("Product")).unwrap();
    assert_eq!(result.drafts.len(), 3);
    assert_eq!(result.drafts[0].name, "Budget threshold alert");
    assert_eq!(result.drafts[1].name, "Persist alert preferences");
    assert_eq!(result.drafts[2].name, "Verify alert trigger coverage");
    assert!(result.drafts[1].prompt.contains("refine model"));
    assert!(result.drafts[2].prompt.contains("automated tests"));
}

#[test]
fn plan_import_prompt_excludes_refine_from_feature_metadata_contract() {
    let prompt = import_extraction_prompt("Personal Budget App\nTrack expenses.", "plan");
    assert!(prompt.contains("feature"));
    assert!(prompt.contains("implementation_goals"));
    assert!(prompt.contains("independently reviewable Goals"));
    assert!(prompt.contains("architecture"));
    assert!(prompt.contains("verification"));
    assert!(prompt.contains("not Refine or this extraction"));
}

#[test]
fn feature_spec_import_prompt_uses_architecture_lenses() {
    let prompt = import_extraction_prompt("Build a budget app.", "feature import");
    assert!(prompt.contains("Plan or feature spec"));
    assert!(prompt.contains("architecture"));
    assert!(prompt.contains("implementation order"));
    assert!(prompt.contains("only for prerequisites"));
}
