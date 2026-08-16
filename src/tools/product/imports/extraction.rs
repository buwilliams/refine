use serde::{Deserialize, Serialize};

use crate::structured_output::Contract;

use super::*;

/// The canonical Feature-import shape shown to extraction agents. The draft
/// parser deliberately accepts more spellings (see `import_draft_from_value`),
/// but the prompt teaches exactly this one.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ImportFeatureContract {
    feature: ImportFeatureMetaContract,
    drafts: Vec<ImportDraftContract>,
    implementation_goals: Vec<ImportDraftContract>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ImportFeatureMetaContract {
    name: String,
    description: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ImportDraftContract {
    name: String,
    prompt: String,
    reporter: String,
    priority: String,
    depends_on: Vec<String>,
}

impl Contract for ImportFeatureContract {
    const LABEL: &'static str = "Feature import JSON";

    fn example() -> Self {
        let draft = |priority: &str| ImportDraftContract {
            name: "...".to_string(),
            prompt: "...".to_string(),
            reporter: String::new(),
            priority: priority.to_string(),
            depends_on: Vec::new(),
        };
        ImportFeatureContract {
            feature: ImportFeatureMetaContract {
                name: "...".to_string(),
                description: "...".to_string(),
            },
            drafts: vec![draft("low")],
            implementation_goals: vec![draft("medium")],
        }
    }
}

pub fn import_extraction_prompt(text: &str, purpose: &str) -> String {
    let template = match purpose {
        "plan" | "feature import" | "feature_spec" | "feature-spec" | "spec" => {
            PromptTemplate::ImportFeature
        }
        "round" => PromptTemplate::ImportRound,
        "plan_goal" | "plan-goal" => PromptTemplate::ImportPlanGoal,
        "standalone_goal" => PromptTemplate::ImportStandaloneGoal,
        _ => PromptTemplate::ImportNotes,
    };
    if matches!(template, PromptTemplate::ImportFeature) {
        let feature_contract = ImportFeatureContract::contract_json();
        return render(
            template,
            &[("text", text), ("feature_contract", &feature_contract)],
        );
    }
    render(template, &[("text", text)])
}

pub fn parse_provider_import_result(
    output: &str,
    reporter: Option<&str>,
) -> RefineResult<ImportExtractionResult> {
    if let Some(result) = parse_structured_import_result(output, reporter) {
        return Ok(result);
    }

    FileImportService::new(PathBuf::new())
        .parse_text(output, reporter)
        .map(|drafts| ImportExtractionResult {
            drafts,
            feature_destination: None,
        })
}

pub fn parse_structured_import_result(
    output: &str,
    reporter: Option<&str>,
) -> Option<ImportExtractionResult> {
    let candidates = crate::structured_output::json_candidates(
        output,
        &crate::structured_output::DecodeOptions::new("import drafts JSON"),
    );
    for value in candidates {
        if let Some(result) = import_extraction_from_json_value(value, reporter) {
            return Some(result);
        }
    }

    let json_lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>();
    if let Ok(items) = json_lines
        && !items.is_empty()
    {
        let body = json!({ "drafts": items, "reporter": reporter.unwrap_or("") });
        if let Ok(drafts) = import_drafts_from_value(&body, reporter) {
            return Some(ImportExtractionResult {
                drafts,
                feature_destination: None,
            });
        }
    }

    None
}

pub(super) fn import_extraction_from_json_value(
    value: Value,
    reporter: Option<&str>,
) -> Option<ImportExtractionResult> {
    let feature_destination = plan_feature_destination_from_value(&value);
    let mut collected = Vec::new();
    collect_import_draft_values(&value, &mut collected, false);

    let body = if collected.is_empty() {
        match value {
            Value::Array(items) => json!({ "drafts": items, "reporter": reporter.unwrap_or("") }),
            Value::Object(mut object) => {
                normalize_plan_feature_draft_object(&mut object);
                Value::Object(object)
            }
            other => other,
        }
    } else {
        json!({ "drafts": collected, "reporter": reporter.unwrap_or("") })
    };

    import_drafts_from_value(&body, reporter)
        .ok()
        .map(|drafts| ImportExtractionResult {
            drafts,
            feature_destination,
        })
}

pub(super) fn normalize_plan_feature_draft_object(object: &mut Map<String, Value>) {
    let mut drafts = Vec::new();
    append_plan_goal_arrays(object, &mut drafts);
    if let Some(feature) = object.get("feature").and_then(Value::as_object) {
        append_plan_goal_arrays(feature, &mut drafts);
    }
    if !drafts.is_empty() {
        object.insert("drafts".to_string(), Value::Array(drafts));
    }
}

pub(super) fn append_plan_goal_arrays(object: &Map<String, Value>, drafts: &mut Vec<Value>) {
    for key in GOAL_ARRAY_KEYS {
        if let Some(items) = object.get(*key).and_then(Value::as_array) {
            drafts.extend(items.iter().cloned());
        }
    }
}

pub(super) fn collect_import_draft_values(
    value: &Value,
    drafts: &mut Vec<Value>,
    in_goal_array: bool,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_import_draft_values(item, drafts, in_goal_array);
            }
        }
        Value::Object(object) => {
            if in_goal_array || is_import_draft_object(object) {
                drafts.push(Value::Object(object.clone()));
                return;
            }
            for (key, child) in object {
                if GOAL_ARRAY_KEYS.contains(&key.as_str()) {
                    if let Value::Array(items) = child {
                        for item in items {
                            collect_import_draft_values(item, drafts, true);
                        }
                    }
                    continue;
                }
                if should_descend_import_container(key, child) {
                    collect_import_draft_values(child, drafts, false);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn feature_import_contract_example_roundtrips_and_yields_drafts() {
        crate::structured_output::assert_contract_roundtrip::<ImportFeatureContract>();
        let result =
            parse_structured_import_result(&ImportFeatureContract::contract_json(), Some("QA"))
                .expect("contract example must parse as an import");
        assert_eq!(result.drafts.len(), 2);
    }
}
