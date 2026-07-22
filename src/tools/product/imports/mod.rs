use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};
use crate::tools::product::work_items::FileWorkItemService;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImportDraft {
    pub name: String,
    pub prompt: String,
    pub reporter: String,
    #[serde(default)]
    pub assignee: Option<String>,
    pub priority: String,
    #[serde(default)]
    pub duplicate_decision: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_names: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportPersistResult {
    pub created: usize,
    pub goal_ids: Vec<String>,
    pub feature_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportExtractionResult {
    pub drafts: Vec<ImportDraft>,
    pub feature_destination: Option<PlanFeatureDestination>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanFeatureDestination {
    pub name: String,
    pub description: String,
}

pub fn validate_import_extraction_result(
    mut result: ImportExtractionResult,
    purpose: &str,
) -> RefineResult<ImportExtractionResult> {
    if purpose == "plan" && result.drafts.is_empty() {
        return Err(RefineError::InvalidInput(
            "Plan Draft extraction did not return any Goal drafts".to_string(),
        ));
    }
    if matches!(purpose, "plan_goal" | "plan-goal") {
        if result.drafts.len() != 1 {
            return Err(RefineError::InvalidInput(
                "Plan Goal extraction must return exactly one Goal draft".to_string(),
            ));
        }
        result.feature_destination = None;
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct FileImportService {
    pub refine_dir: PathBuf,
}

impl FileImportService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn parse_text(&self, text: &str, reporter: Option<&str>) -> RefineResult<Vec<ImportDraft>> {
        let drafts = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| ImportDraft {
                name: import_name("", line),
                prompt: line.to_string(),
                reporter: reporter.unwrap_or("").trim().to_string(),
                assignee: reporter
                    .map(str::trim)
                    .filter(|reporter| !reporter.is_empty())
                    .map(str::to_string),
                priority: "low".to_string(),
                duplicate_decision: String::new(),
                dependency_names: Vec::new(),
            })
            .collect::<Vec<_>>();
        Ok(drafts)
    }

    pub fn parse_csv(&self, text: &str, reporter: Option<&str>) -> RefineResult<Vec<ImportDraft>> {
        let rows = parse_csv_rows(text)?;
        let Some(headers) = rows.first() else {
            return Ok(Vec::new());
        };
        let headers: Vec<String> = headers
            .iter()
            .map(|header| header.trim().to_lowercase())
            .collect();
        let mut drafts = Vec::new();
        for (row_index, columns) in rows.iter().enumerate().skip(1) {
            if columns.iter().all(|cell| cell.trim().is_empty()) {
                continue;
            }
            let value = |name: &str| {
                headers
                    .iter()
                    .position(|header| header == name)
                    .and_then(|index| columns.get(index))
                    .map(String::as_str)
                    .unwrap_or("")
                    .trim()
            };
            let prompt = value("prompt");
            if prompt.is_empty() {
                continue;
            }
            let priority = normalized_priority(value("priority")).map_err(|_| {
                RefineError::InvalidInput(format!(
                    "CSV row {} priority must be one of low, medium, or high",
                    row_index + 1
                ))
            })?;
            drafts.push(ImportDraft {
                name: import_name(value("name"), prompt),
                prompt: prompt.to_string(),
                reporter: nonempty_or(value("reporter"), reporter.unwrap_or("")).to_string(),
                assignee: Some(
                    nonempty_or(
                        value("assignee"),
                        nonempty_or(value("reporter"), reporter.unwrap_or("")),
                    )
                    .to_string(),
                )
                .filter(|assignee| !assignee.is_empty()),
                priority,
                duplicate_decision: String::new(),
                dependency_names: Vec::new(),
            });
        }
        Ok(drafts)
    }

    pub fn parse_structured_or_text(
        &self,
        text: &str,
        reporter: Option<&str>,
    ) -> RefineResult<Vec<ImportDraft>> {
        parse_provider_import_result(text, reporter).map(|result| result.drafts)
    }

    pub fn import_from_text(
        &self,
        text: &str,
        csv: bool,
        reporter: Option<&str>,
        feature_id: Option<&str>,
    ) -> RefineResult<ImportPersistResult> {
        let drafts = if csv {
            self.parse_csv(text, reporter)?
        } else {
            self.parse_structured_or_text(text, reporter)?
        };
        if drafts.is_empty() {
            return Err(RefineError::InvalidInput(
                "import input did not contain any drafts".to_string(),
            ));
        }
        self.persist(drafts, feature_id)
    }

    pub fn import_from_file(
        &self,
        path: impl Into<PathBuf>,
        csv: bool,
        reporter: Option<&str>,
        feature_id: Option<&str>,
    ) -> RefineResult<ImportPersistResult> {
        let path = path.into();
        let text = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read import file {}: {error}",
                path.display()
            ))
        })?;
        self.import_from_text(&text, csv, reporter, feature_id)
    }

    pub fn persist(
        &self,
        drafts: Vec<ImportDraft>,
        feature_id: Option<&str>,
    ) -> RefineResult<ImportPersistResult> {
        let work_items = FileWorkItemService::new(&self.refine_dir);
        let mut goal_ids = Vec::new();
        let mut created_drafts = Vec::new();
        if let Some(feature_id) = feature_id {
            work_items.show_feature_summary(feature_id)?;
        }
        for draft in drafts {
            let goal = work_items.create_goal_summary(&draft.name, None)?;
            if !draft.prompt.trim().is_empty() {
                work_items.append_goal_round_summary_with_assignee(
                    &goal.goal.id,
                    nonempty_or(&draft.reporter, "Imported"),
                    draft.assignee.as_deref(),
                    &draft.prompt,
                )?;
            }
            if goal.goal.priority.as_str() != draft.priority || !draft.reporter.trim().is_empty() {
                work_items.update_goal_metadata_summary(
                    &goal.goal.id,
                    None,
                    (goal.goal.priority.as_str() != draft.priority)
                        .then_some(draft.priority.as_str()),
                    nonempty_option(&draft.reporter),
                    None,
                )?;
            }
            if let Some(feature_id) = feature_id {
                work_items.assign_goal_to_feature(feature_id, &goal.goal.id)?;
            }
            goal_ids.push(goal.goal.id.clone());
            created_drafts.push((draft, goal.goal.id));
        }
        if let Some(feature_id) = feature_id {
            order_feature_dependency_drafts(&work_items, feature_id, &created_drafts)?;
        }
        Ok(ImportPersistResult {
            created: goal_ids.len(),
            goal_ids,
            feature_id: feature_id.map(str::to_string),
        })
    }
}

pub fn import_drafts_from_value(
    body: &serde_json::Value,
    default_reporter: Option<&str>,
) -> RefineResult<Vec<ImportDraft>> {
    let default_reporter = body
        .get("reporter")
        .and_then(|value| value.as_str())
        .or(default_reporter)
        .unwrap_or("")
        .trim();
    let drafts = body
        .get("drafts")
        .or_else(|| body.get("items"))
        .unwrap_or(body);
    let Some(drafts) = drafts.as_array() else {
        return Err(RefineError::InvalidInput(
            "body.drafts must be an array".to_string(),
        ));
    };
    drafts
        .iter()
        .enumerate()
        .map(|(index, value)| import_draft_from_value(value, default_reporter, index + 1))
        .collect()
}

fn import_draft_from_value(
    value: &serde_json::Value,
    default_reporter: &str,
    index: usize,
) -> RefineResult<ImportDraft> {
    let Some(object) = value.as_object() else {
        return Err(RefineError::InvalidInput(format!(
            "draft {index} must be an object"
        )));
    };
    let field = |key: &str| -> &str { string_field(object, &[key]) };
    let prompt = field("prompt").to_string();
    let priority = normalized_priority(field("priority")).map_err(|_| {
        RefineError::InvalidInput(format!(
            "draft {index} priority must be one of low, medium, or high"
        ))
    })?;
    let reporter = nonempty_or(field("reporter"), default_reporter).to_string();
    let assignee = nonempty_or(field("assignee"), &reporter).to_string();
    Ok(ImportDraft {
        name: import_name(string_field(object, &["name", "title", "summary"]), &prompt),
        prompt,
        reporter,
        assignee: (!assignee.is_empty()).then_some(assignee),
        priority,
        duplicate_decision: field("duplicate_decision").to_string(),
        dependency_names: string_list_field(
            object,
            &[
                "dependency_names",
                "depends_on",
                "dependencies",
                "after",
                "requires",
            ],
        ),
    })
}

pub fn order_feature_dependency_drafts(
    work_items: &FileWorkItemService,
    feature_id: &str,
    created_drafts: &[(ImportDraft, String)],
) -> RefineResult<()> {
    let ordered_goal_ids = dependency_ordered_goal_ids(created_drafts);
    if !ordered_goal_ids.is_empty() {
        work_items.order_goals_in_feature(feature_id, &ordered_goal_ids)?;
    }
    Ok(())
}

fn dependency_ordered_goal_ids(created_drafts: &[(ImportDraft, String)]) -> Vec<String> {
    let mut name_to_goal_id = BTreeMap::new();
    let mut position_by_goal_id = BTreeMap::new();
    for (index, (draft, goal_id)) in created_drafts.iter().enumerate() {
        position_by_goal_id.insert(goal_id.clone(), index);
        for key in [&draft.name, goal_id] {
            let key = normalize_dependency_key(key);
            if !key.is_empty() {
                name_to_goal_id.insert(key, goal_id.clone());
            }
        }
    }

    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut involved = BTreeSet::new();
    for (draft, goal_id) in created_drafts {
        for dependency in &draft.dependency_names {
            let dependency_key = normalize_dependency_key(dependency);
            let Some(prerequisite_id) = name_to_goal_id.get(&dependency_key) else {
                continue;
            };
            if prerequisite_id == goal_id {
                continue;
            }
            edges
                .entry(prerequisite_id.clone())
                .or_default()
                .insert(goal_id.clone());
            involved.insert(prerequisite_id.clone());
            involved.insert(goal_id.clone());
        }
    }
    if involved.is_empty() {
        return Vec::new();
    }

    let mut incoming: BTreeMap<String, usize> = involved
        .iter()
        .map(|goal_id| (goal_id.clone(), 0usize))
        .collect();
    for dependents in edges.values() {
        for dependent in dependents {
            if let Some(count) = incoming.get_mut(dependent) {
                *count += 1;
            }
        }
    }

    let mut ordered = Vec::new();
    while let Some(next_id) = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .min_by_key(|(goal_id, _)| {
            position_by_goal_id
                .get(*goal_id)
                .copied()
                .unwrap_or(usize::MAX)
        })
        .map(|(goal_id, _)| goal_id.clone())
    {
        incoming.remove(&next_id);
        ordered.push(next_id.clone());
        if let Some(dependents) = edges.get(&next_id) {
            for dependent in dependents {
                if let Some(count) = incoming.get_mut(dependent) {
                    *count = count.saturating_sub(1);
                }
            }
        }
    }

    if !incoming.is_empty() {
        let mut fallback = involved.into_iter().collect::<Vec<_>>();
        fallback.sort_by_key(|goal_id| {
            position_by_goal_id
                .get(goal_id)
                .copied()
                .unwrap_or(usize::MAX)
        });
        return fallback;
    }
    ordered
}

fn parse_csv_rows(text: &str) -> RefineResult<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                row.push(cell.trim().to_string());
                cell.clear();
            }
            '\n' if !quoted => {
                row.push(cell.trim_end_matches('\r').trim().to_string());
                cell.clear();
                rows.push(row);
                row = Vec::new();
            }
            _ => cell.push(ch),
        }
    }
    if quoted {
        return Err(RefineError::InvalidInput(
            "CSV contains an unclosed quoted field".to_string(),
        ));
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell.trim_end_matches('\r').trim().to_string());
        rows.push(row);
    }
    Ok(rows)
}

fn import_name(name: &str, prompt: &str) -> String {
    let raw = [name.trim(), prompt.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("Imported Goal");
    let mut result: String = raw.chars().take(80).collect();
    if result.trim().is_empty() {
        result = "Imported Goal".to_string();
    }
    result
}

fn normalized_priority(priority: &str) -> RefineResult<String> {
    let priority = priority.trim().to_lowercase();
    let priority = if priority.is_empty() {
        "low".to_string()
    } else {
        priority
    };
    match priority.as_str() {
        "low" | "medium" | "high" => Ok(priority),
        _ => Err(RefineError::InvalidInput(
            "priority must be one of low, medium, or high".to_string(),
        )),
    }
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

fn nonempty_option(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
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
    if let Ok(value) = serde_json::from_str::<Value>(output)
        && let Some(result) = import_extraction_from_json_value(value, reporter)
    {
        return Some(result);
    }

    if let Some(result) = embedded_json_import_extraction(output, reporter) {
        return Some(result);
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

fn embedded_json_import_extraction(
    output: &str,
    reporter: Option<&str>,
) -> Option<ImportExtractionResult> {
    for (idx, ch) in output.char_indices() {
        if ch != '{' && ch != '[' {
            continue;
        }
        let mut values = serde_json::Deserializer::from_str(&output[idx..]).into_iter::<Value>();
        let Some(Ok(value)) = values.next() else {
            continue;
        };
        if let Some(result) = import_extraction_from_json_value(value, reporter) {
            return Some(result);
        }
    }
    None
}

fn import_extraction_from_json_value(
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

fn normalize_plan_feature_draft_object(object: &mut Map<String, Value>) {
    let mut drafts = Vec::new();
    append_plan_goal_arrays(object, &mut drafts);
    if let Some(feature) = object.get("feature").and_then(Value::as_object) {
        append_plan_goal_arrays(feature, &mut drafts);
    }
    if !drafts.is_empty() {
        object.insert("drafts".to_string(), Value::Array(drafts));
    }
}

fn append_plan_goal_arrays(object: &Map<String, Value>, drafts: &mut Vec<Value>) {
    for key in GOAL_ARRAY_KEYS {
        if let Some(items) = object.get(*key).and_then(Value::as_array) {
            drafts.extend(items.iter().cloned());
        }
    }
}

fn collect_import_draft_values(value: &Value, drafts: &mut Vec<Value>, in_goal_array: bool) {
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

const GOAL_ARRAY_KEYS: &[&str] = &[
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

fn should_descend_import_container(key: &str, value: &Value) -> bool {
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

fn is_import_draft_object(object: &Map<String, Value>) -> bool {
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

fn has_nested_goal_arrays(object: &Map<String, Value>) -> bool {
    GOAL_ARRAY_KEYS
        .iter()
        .any(|key| object.get(*key).and_then(Value::as_array).is_some())
        || object.get("features").and_then(Value::as_array).is_some()
}

fn object_has_any(object: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| object.contains_key(*key))
}

fn string_field<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
}

fn string_list_field(object: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| object.get(*key))
        .map(string_list_value)
        .unwrap_or_default()
}

fn string_list_value(value: &Value) -> Vec<String> {
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

fn split_dependency_names(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split([',', '\n', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_dependency_key(value: &str) -> String {
    collapse_ws(value).to_lowercase()
}

fn plan_feature_destination_from_value(value: &Value) -> Option<PlanFeatureDestination> {
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

fn sanitize_plan_feature_name(raw: &str) -> String {
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

fn sanitize_plan_feature_description(raw: &str) -> String {
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

fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_feature_text(value: String, max_chars: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
