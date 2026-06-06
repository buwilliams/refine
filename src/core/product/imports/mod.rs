use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::product::work_items::FileWorkItemService;
use crate::core::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImportDraft {
    pub name: String,
    pub actual: String,
    pub target: String,
    pub reporter: String,
    pub priority: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportPersistResult {
    pub created: usize,
    pub gap_ids: Vec<String>,
    pub feature_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileImportService {
    pub durable_root: PathBuf,
}

impl FileImportService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
        }
    }

    pub fn parse_text(&self, text: &str, reporter: Option<&str>) -> RefineResult<Vec<ImportDraft>> {
        let drafts = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (actual, target) = split_import_line(line);
                ImportDraft {
                    name: import_name("", actual, target),
                    actual: actual.to_string(),
                    target: target.to_string(),
                    reporter: reporter.unwrap_or("").trim().to_string(),
                    priority: "low".to_string(),
                }
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
            let actual = value("actual");
            let target = value("target");
            if actual.is_empty() && target.is_empty() {
                continue;
            }
            let priority = normalized_priority(value("priority")).map_err(|_| {
                RefineError::InvalidInput(format!(
                    "CSV row {} priority must be one of low, medium, or high",
                    row_index + 1
                ))
            })?;
            drafts.push(ImportDraft {
                name: import_name(value("name"), actual, target),
                actual: actual.to_string(),
                target: target.to_string(),
                reporter: nonempty_or(value("reporter"), reporter.unwrap_or("")).to_string(),
                priority,
            });
        }
        Ok(drafts)
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
            self.parse_text(text, reporter)?
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
        let work_items = FileWorkItemService::new(&self.durable_root);
        let mut gap_ids = Vec::new();
        if let Some(feature_id) = feature_id {
            work_items.show_feature_summary(feature_id)?;
        }
        for draft in drafts {
            let gap = work_items.create_gap_summary(&draft.name, None)?;
            if !draft.actual.trim().is_empty() || !draft.target.trim().is_empty() {
                work_items.append_gap_round_summary(
                    &gap.gap.id,
                    nonempty_or(&draft.reporter, "Imported"),
                    &draft.actual,
                    &draft.target,
                )?;
            }
            if gap.gap.priority.as_str() != draft.priority {
                work_items.update_gap_metadata_summary(&gap.gap.id, None, Some(&draft.priority))?;
            }
            if let Some(feature_id) = feature_id {
                work_items.assign_gap_to_feature(feature_id, &gap.gap.id)?;
            }
            gap_ids.push(gap.gap.id);
        }
        Ok(ImportPersistResult {
            created: gap_ids.len(),
            gap_ids,
            feature_id: feature_id.map(str::to_string),
        })
    }
}

fn split_import_line(line: &str) -> (&str, &str) {
    line.split_once("=>")
        .or_else(|| line.split_once("->"))
        .or_else(|| line.split_once('|'))
        .map(|(actual, target)| (actual.trim(), target.trim()))
        .unwrap_or((line.trim(), ""))
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
    let field = |key: &str| -> &str {
        object
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
    };
    let actual = field("actual").to_string();
    let target = field("target").to_string();
    let priority = normalized_priority(field("priority")).map_err(|_| {
        RefineError::InvalidInput(format!(
            "draft {index} priority must be one of low, medium, or high"
        ))
    })?;
    Ok(ImportDraft {
        name: import_name(field("name"), &actual, &target),
        actual,
        target,
        reporter: nonempty_or(field("reporter"), default_reporter).to_string(),
        priority,
    })
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

fn import_name(name: &str, actual: &str, target: &str) -> String {
    let raw = [name.trim(), target.trim(), actual.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("Imported Gap");
    let mut result: String = raw.chars().take(80).collect();
    if result.trim().is_empty() {
        result = "Imported Gap".to_string();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_import_service_imports_text_into_feature() {
        let temp_root = unique_temp_dir("import");
        let durable_root = temp_root.join(".refine");
        FileWorkItemService::new(&durable_root)
            .create_feature_summary("Feature", Some("FEA1"), None, None)
            .unwrap();

        let result = FileImportService::new(&durable_root)
            .import_from_text(
                "Actual behavior => Target behavior",
                false,
                Some("Reporter"),
                Some("FEA1"),
            )
            .unwrap();

        assert_eq!(result.created, 1);
        let gap = FileWorkItemService::new(&durable_root)
            .show_gap_summary(&result.gap_ids[0])
            .unwrap();
        assert_eq!(gap.gap.feature_id.as_deref(), Some("FEA1"));
        assert_eq!(gap.gap.reporter.as_deref(), Some("Reporter"));

        std::fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
