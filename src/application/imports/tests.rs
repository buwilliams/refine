use super::*;
use crate::application::work_items::FileWorkItemService;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn file_import_service_imports_text_into_feature() {
    let temp_root = unique_temp_dir("import");
    let refine_dir = temp_root.join(".refine");
    FileWorkItemService::new(&refine_dir)
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();

    let result = FileImportService::new(&refine_dir)
        .import_from_text(
            "Actual behavior => Target behavior",
            false,
            Some("Reporter"),
            Some("FEA1"),
        )
        .unwrap();

    assert_eq!(result.created, 1);
    let goal = FileWorkItemService::new(&refine_dir)
        .show_goal_summary(&result.goal_ids[0])
        .unwrap();
    assert_eq!(goal.goal.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(goal.goal.feature_order, None);
    assert_eq!(goal.goal.reporter.as_deref(), Some("Reporter"));

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn plan_goal_extraction_prompt_requests_one_goal_without_a_feature() {
    let prompt = import_extraction_prompt("Plan transcript", "plan_goal");

    assert!(prompt.contains("one independently actionable Goal"));
    assert!(prompt.contains("implementation and verification context"));
    assert!(prompt.contains("no Feature, dependencies, or commentary"));
    assert!(prompt.ends_with("\n\nPlan transcript"));
}

#[test]
fn file_import_service_orders_only_dependency_connected_feature_goals() {
    let temp_root = unique_temp_dir("import-dependencies");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();

    let result = FileImportService::new(&refine_dir)
        .import_from_text(
            &json!({
                "drafts": [
                    {
                        "name": "Create saved list",
                        "prompt": "Create a saved list.",
                        "priority": "medium"
                    },
                    {
                        "name": "Sort saved list",
                        "prompt": "Let users sort the saved list.",
                        "priority": "medium",
                        "depends_on": ["Create saved list"]
                    },
                    {
                        "name": "Tune empty state",
                        "prompt": "Make the empty state product-specific.",
                        "priority": "low"
                    }
                ]
            })
            .to_string(),
            false,
            None,
            Some("FEA1"),
        )
        .unwrap();

    let work_items = FileWorkItemService::new(&refine_dir);
    let goals = result
        .goal_ids
        .iter()
        .map(|id| work_items.show_goal_summary(id).unwrap().goal)
        .collect::<Vec<_>>();
    assert_eq!(goals[0].feature_order, Some(1));
    assert_eq!(goals[1].feature_order, Some(2));
    assert_eq!(goals[2].feature_order, None);

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn provider_import_result_flattens_nested_project_features_into_goals() {
    let output = json!({
        "project": {
            "name": "Personal Budget App — Product Spec",
            "purpose": "Help users track spending and budgets.",
            "features": [
                {
                    "name": "Transaction Tracking",
                    "goals": [
                        {
                            "title": "Categorize transactions",
                            "prompt": "Let users assign each imported transaction to a category.",
                            "priority": "medium"
                        }
                    ]
                },
                {
                    "name": "Budget Alerts",
                    "implementation_goals": [
                        {
                            "name": "Persist alert preferences",
                            "prompt": "Persist alert thresholds per category.",
                            "priority": "high"
                        }
                    ]
                }
            ]
        }
    })
    .to_string();

    let result = parse_provider_import_result(&output, Some("Product")).unwrap();

    assert_eq!(result.drafts.len(), 2);
    assert_eq!(result.drafts[0].name, "Categorize transactions");
    assert_eq!(
        result.drafts[0].prompt,
        "Let users assign each imported transaction to a category."
    );
    assert_eq!(result.drafts[0].reporter, "Product");
    assert_eq!(result.drafts[1].name, "Persist alert preferences");
    let feature = result.feature_destination.unwrap();
    assert_eq!(feature.name, "Personal Budget App");
    assert_eq!(
        feature.description,
        "Help users track spending and budgets."
    );
}

#[test]
fn provider_import_result_keeps_feature_wrapper_out_of_drafts() {
    let output = json!({
        "features": [
            {
                "name": "User Profiles",
                "description": "Manage profile details.",
                "goals": [
                    {
                        "name": "Profile editor",
                        "prompt": "Let users update profile details.",
                        "priority": "low"
                    }
                ]
            }
        ]
    })
    .to_string();

    let result = parse_provider_import_result(&output, None).unwrap();

    assert_eq!(result.drafts.len(), 1);
    assert_eq!(result.drafts[0].name, "Profile editor");
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
