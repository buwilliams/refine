use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::application::work_items::{BulkGoalSelection, FileWorkItemService};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitChange};

mod budgeting;
mod rendering;
mod selection;
mod summaries;
#[cfg(test)]
mod tests;

use budgeting::*;
use rendering::*;
use selection::*;
use summaries::*;

const JIRA_DESCRIPTION_LIMIT: usize = 30_000;
const IDENTITY_RESERVE: usize = 2_500;
const TRACEABILITY_RESERVE: usize = 3_000;
const COMMIT_RESERVE: usize = 3_500;
const OUTCOME_RESERVE: usize = 7_000;
const REQUEST_RESERVE: usize = 5_000;
const IMPLEMENTATION_RESERVE: usize = 5_000;
const GUIDANCE_RESERVE: usize = 1_200;
const NOTES_RESERVE: usize = 1_800;
const JIRA_HEADERS: [&str; 10] = [
    "Summary",
    "Description",
    "Work Type",
    "Priority",
    "Labels",
    "Refine Goal ID",
    "Refine Status",
    "Refine Branch",
    "Base Commit",
    "Candidate Commit",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraGoalExport {
    pub format: String,
    pub filename: String,
    pub content_type: String,
    pub goal_id: String,
    pub commit_count: usize,
    pub csv: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraGoalsExport {
    pub format: String,
    pub filename: String,
    pub content_type: String,
    pub goal_ids: Vec<String>,
    pub goal_count: usize,
    pub commit_count: usize,
    pub csv: String,
}

#[derive(Clone, Debug)]
pub struct FileGoalExportService {
    refine_dir: PathBuf,
    target_root: PathBuf,
    runtime_root: Option<PathBuf>,
    operation_id: Option<String>,
}

impl FileGoalExportService {
    pub fn new(refine_dir: impl Into<PathBuf>, target_root: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            target_root: target_root.into(),
            runtime_root: None,
            operation_id: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            target_root: target_root.into(),
            runtime_root: Some(runtime_root.into()),
            operation_id: None,
        }
    }

    pub fn with_operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn export_jira_csv(&self, goal_id: &str) -> RefineResult<JiraGoalExport> {
        let work_items = self.work_items();
        let goal = work_items.show_goal_detail(goal_id)?;
        let commits = self.goal_commits(&goal)?;
        jira_export_from_goal(&goal, &commits)
    }

    pub fn export_bulk_jira_csv(
        &self,
        selection: &BulkGoalSelection,
    ) -> RefineResult<JiraGoalsExport> {
        self.export_bulk_jira_csv_with_progress(selection, |_, _, _| Ok(()))
    }

    pub fn export_bulk_jira_csv_with_progress<F>(
        &self,
        selection: &BulkGoalSelection,
        mut report: F,
    ) -> RefineResult<JiraGoalsExport>
    where
        F: FnMut(&str, usize, usize) -> RefineResult<()>,
    {
        let work_items = self.work_items();
        let goal_ids = work_items
            .select_bulk_goal_ids(selection)?
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if goal_ids.is_empty() {
            return Err(RefineError::InvalidInput(
                "Select at least one Goal to export for Jira".to_string(),
            ));
        }

        let goal_total = goal_ids.len();
        report("Loading selected Goal evidence", 0, goal_total)?;
        let mut goals = Vec::with_capacity(goal_total);
        for (index, goal_id) in goal_ids.iter().enumerate() {
            goals.push(work_items.show_goal_detail(goal_id)?);
            report("Loading selected Goal evidence", index + 1, goal_total)?;
        }

        let ranges = goals
            .iter()
            .filter_map(goal_commit_range)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        report("Looking up commit evidence", 0, goal_total)?;
        let commits_by_range = self.git().changes_between_many(&ranges)?;

        let mut rows = Vec::with_capacity(goal_ids.len());
        let mut commit_count = 0;
        for (index, goal) in goals.iter().enumerate() {
            let commits = commits_for_goal(goal, &commits_by_range);
            commit_count += commits.len();
            rows.push(jira_row(goal, commits)?);
            report("Building Jira CSV", index + 1, goal_total)?;
        }
        let csv = format!("{}\r\n{}\r\n", JIRA_HEADERS.join(","), rows.join("\r\n"));

        Ok(JiraGoalsExport {
            format: "jira_csv".to_string(),
            filename: "refine-goals-jira.csv".to_string(),
            content_type: "text/csv; charset=utf-8".to_string(),
            goal_count: goal_ids.len(),
            goal_ids,
            commit_count,
            csv,
        })
    }

    fn work_items(&self) -> FileWorkItemService {
        match &self.runtime_root {
            Some(runtime_root) => FileWorkItemService::with_projection_cache(
                &self.refine_dir,
                runtime_root,
                runtime_root.join("cache"),
            ),
            None => FileWorkItemService::new(&self.refine_dir),
        }
    }

    fn goal_commits(&self, goal: &Value) -> RefineResult<Vec<GitChange>> {
        let Some(range) = goal_commit_range(goal) else {
            return Ok(Vec::new());
        };
        self.git().changes_between(&range.0, &range.1)
    }

    fn git(&self) -> FileGitWorktreeService {
        let git = match &self.runtime_root {
            Some(runtime_root) => {
                FileGitWorktreeService::with_runtime_root(&self.target_root, runtime_root)
            }
            None => FileGitWorktreeService::new(&self.target_root),
        };
        match &self.operation_id {
            Some(operation_id) => git.with_operation_id(operation_id),
            None => git,
        }
    }
}
