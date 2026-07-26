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

mod csv;
mod drafts;
mod extraction;
mod normalization;

pub use drafts::{import_drafts_from_value, order_feature_dependency_drafts};
pub use extraction::{
    import_extraction_prompt, parse_provider_import_result, parse_structured_import_result,
};

use csv::*;
use normalization::*;

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

#[cfg(test)]
mod tests;
