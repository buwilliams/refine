use std::path::{Path, PathBuf};

use serde_json::json;

use crate::model::JsonObject;
use crate::model::log::LogEntry;
use crate::model::workflow::GapStatus;
use crate::process::subprocess::workflow_subprocess_metadata;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::MergeResult;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::{WorkflowClaim, json_object, now_timestamp};

pub struct WorkflowContext<'a> {
    pub runtime_root: &'a Path,
    pub durable_root: &'a Path,
    pub app_root: &'a Path,
    pub claim_id: String,
    pub gap_id: String,
    pub provider: String,
    pub execution_id: &'a str,
    pub round_idx: usize,
    pub settings: JsonObject,
    pub work_items: FileWorkItemService,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub agent_cwd: Option<PathBuf>,
    pub provider_output: Option<String>,
    pub commit: Option<String>,
    pub merge: Option<MergeResult>,
    pub final_status: Option<GapStatus>,
}

impl<'a> WorkflowContext<'a> {
    pub fn new(
        runtime_root: &'a Path,
        durable_root: &'a Path,
        app_root: &'a Path,
        claim: WorkflowClaim,
        execution_id: &'a str,
        round_idx: usize,
        settings: JsonObject,
        work_items: FileWorkItemService,
    ) -> Self {
        Self {
            runtime_root,
            durable_root,
            app_root,
            claim_id: claim.claim_id,
            gap_id: claim.gap_id,
            provider: claim.provider,
            execution_id,
            round_idx,
            settings,
            work_items,
            branch: None,
            worktree_path: None,
            agent_cwd: None,
            provider_output: None,
            commit: None,
            merge: None,
            final_status: None,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        self.runtime_root
    }

    pub fn durable_root(&self) -> &Path {
        self.durable_root
    }

    pub fn request_transition(&mut self, from: GapStatus, to: GapStatus) -> RefineResult<()> {
        self.work_items
            .advance_automated_gap_status(&self.gap_id, to.clone())?;
        self.log(
            "state",
            &format!(
                "Workflow status changed: {} -> {}",
                from.as_str(),
                to.as_str()
            ),
            None,
        )
    }

    pub fn log(
        &self,
        category: &str,
        message: &str,
        details: Option<JsonObject>,
    ) -> RefineResult<()> {
        let mut details = details.unwrap_or_default();
        details
            .entry("execution_id".to_string())
            .or_insert_with(|| json!(self.execution_id));
        FileLogService::new(self.durable_root).append_round_log(
            &self.gap_id,
            self.round_idx,
            LogEntry {
                datetime: now_timestamp(),
                severity: "info".to_string(),
                category: category.to_string(),
                message: message.to_string(),
                details: Some(details),
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                gap_id: Some(self.gap_id.clone()),
            },
        )?;
        Ok(())
    }

    pub fn fail(&self, category: &str, error: &RefineError) -> RefineResult<()> {
        let _ = self
            .work_items
            .advance_automated_gap_status(&self.gap_id, GapStatus::Failed);
        self.log(
            category,
            &format!("Workflow failed: {error}"),
            Some(json_object(json!({"error": error.to_string()}))),
        )
    }

    pub fn workflow_process_metadata(&self, workflow_state: &str, behavior: &str) -> JsonObject {
        workflow_subprocess_metadata(
            self.execution_id,
            &self.gap_id,
            workflow_state,
            behavior,
            Some(self.round_idx),
        )
    }

    pub fn require_branch(&self) -> RefineResult<&str> {
        self.branch
            .as_deref()
            .ok_or_else(|| missing_artifact("branch", &self.gap_id))
    }

    pub fn require_worktree_path(&self) -> RefineResult<&str> {
        self.worktree_path
            .as_deref()
            .ok_or_else(|| missing_artifact("worktree", &self.gap_id))
    }

    pub fn require_agent_cwd(&self) -> RefineResult<&Path> {
        self.agent_cwd
            .as_deref()
            .ok_or_else(|| missing_artifact("agent cwd", &self.gap_id))
    }

    pub fn require_commit(&self) -> RefineResult<&str> {
        self.commit
            .as_deref()
            .ok_or_else(|| missing_artifact("commit", &self.gap_id))
    }
}

fn missing_artifact(name: &str, gap_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Gap {gap_id}"
    ))
}
