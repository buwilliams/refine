use super::*;

pub(super) const GOAL_ARRAY_KEYS: &[&str] = &[
    "drafts",
    "goals",
    "items",
    "implementation_goals",
    "technical_goals",
    "engineering_goals",
    "backend_goals",
    "frontend_goals",
    "testing_goals",
    "work_goals",
];

pub(super) fn should_descend_import_container(key: &str, value: &Value) -> bool {
    matches!(
        key,
        "feature"
            | "features"
            | "project"
            | "projects"
            | "capability"
            | "capabilities"
            | "module"
            | "modules"
            | "component"
            | "components"
            | "surface"
            | "surfaces"
            | "workflow"
            | "workflows"
            | "workstream"
            | "workstreams"
            | "epic"
            | "epics"
            | "milestone"
            | "milestones"
    ) || matches!(value, Value::Object(_) | Value::Array(_))
}

pub(super) fn is_import_draft_object(object: &Map<String, Value>) -> bool {
    if has_nested_goal_arrays(object) {
        return false;
    }
    object_has_any(object, &["prompt"])
        || object_has_any(object, &["name", "title", "summary"])
            && object_has_any(
                object,
                &[
                    "priority",
                    "reporter",
                    "assignee",
                    "duplicate_decision",
                    "kind",
                    "type",
                ],
            )
}

pub(super) fn has_nested_goal_arrays(object: &Map<String, Value>) -> bool {
    GOAL_ARRAY_KEYS
        .iter()
        .any(|key| object.get(*key).and_then(Value::as_array).is_some())
        || object.get("features").and_then(Value::as_array).is_some()
}

pub(super) fn object_has_any(object: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| object.contains_key(*key))
}

pub(super) fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
}

pub(super) fn string_list_field(object: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .map(string_list_value)
        .unwrap_or_default()
}

pub(super) fn string_list_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_dependency_names)
            .collect(),
        Value::String(value) => split_dependency_names(value).collect(),
        _ => Vec::new(),
    }
}

pub(super) fn split_dependency_names(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split([',', '\n', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn normalize_dependency_key(value: &str) -> String {
    collapse_ws(value).to_lowercase()
}

pub(super) fn plan_feature_destination_from_value(value: &Value) -> Option<PlanFeatureDestination> {
    let feature = match value {
        Value::Object(object) => object
            .get("feature")
            .and_then(Value::as_object)
            .or_else(|| object.get("project").and_then(Value::as_object))
            .or_else(|| {
                object
                    .get("features")
                    .and_then(Value::as_array)
                    .and_then(|features| features.iter().find_map(Value::as_object))
            })
            .or(Some(object)),
        _ => None,
    }?;
    let name = sanitize_plan_feature_name(
        feature
            .get("name")
            .or_else(|| feature.get("feature_name"))
            .or_else(|| feature.get("project_name"))
            .or_else(|| feature.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    let description = sanitize_plan_feature_description(
        feature
            .get("description")
            .or_else(|| feature.get("summary"))
            .or_else(|| feature.get("purpose"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    if name.is_empty() && description.is_empty() {
        return None;
    }
    Some(PlanFeatureDestination { name, description })
}

pub(super) fn sanitize_plan_feature_name(raw: &str) -> String {
    let mut value = collapse_ws(raw);
    for suffix in [
        " - Product Spec",
        " – Product Spec",
        " — Product Spec",
        ": Product Spec",
        " Product Spec",
    ] {
        if value.to_lowercase().ends_with(&suffix.to_lowercase()) {
            value.truncate(value.len().saturating_sub(suffix.len()));
            value = collapse_ws(&value);
        }
    }
    for prefix in ["Product Spec:", "Plan:", "Feature:", "Project:"] {
        if value.to_lowercase().starts_with(&prefix.to_lowercase()) {
            value = collapse_ws(&value[prefix.len()..]);
        }
    }
    trim_feature_text(value, 80)
}

pub(super) fn sanitize_plan_feature_description(raw: &str) -> String {
    let value = collapse_ws(raw);
    let lower = value.to_lowercase();
    if lower.is_empty()
        || lower.contains("created by plan")
        || lower.contains("created from plan")
        || lower.contains("plan mode")
        || lower.contains("refine")
        || lower.contains("product spec")
        || lower.contains("draft")
        || lower.contains("extract")
    {
        return String::new();
    }
    trim_feature_text(value, 500)
}

pub(super) fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn trim_feature_text(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.trim().to_string();
    }
    let mut trimmed = value.chars().take(max_chars).collect::<String>();
    trimmed = trimmed
        .trim_end_matches(|ch: char| !ch.is_alphanumeric())
        .trim()
        .to_string();
    trimmed
}
