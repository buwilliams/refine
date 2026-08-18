use std::path::{Path, PathBuf};

use serde_json::json;

use crate::application::work_items::{FileWorkItemService, WorkflowAttemptAuthority};
use crate::application::workflow::phases::quality::QualityCheckRequest;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::worktrees::MergeResult;
use crate::infrastructure::observability::logs::FileLogService;
use crate::infrastructure::process::subprocess::workflow_subprocess_metadata;
use crate::model::JsonObject;
use crate::model::goal::RoundIntegration;
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;

pub(crate) mod execution;
use crate::application::workflow::{json_object, now_timestamp};

pub struct WorkflowContext<'a> {
    pub runtime_root: &'a Path,
    pub target_root: &'a Path,
    pub goal_id: String,
    pub node_id: String,
    pub provider: String,
    pub round_idx: usize,
    pub(crate) attempt_authority: WorkflowAttemptAuthority,
    pub settings: JsonObject,
    pub work_items: FileWorkItemService,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub candidate_handoff_operation_id: Option<String>,
    pub quality_operation_id: Option<String>,
    pub quality_request: Option<QualityCheckRequest>,
    pub agent_cwd: Option<PathBuf>,
    pub provider_output: Option<String>,
    pub commit: Option<String>,
    pub implementation_changed: bool,
    pub merge: Option<MergeResult>,
    /// Existing integration evidence when a failed round is being reconciled instead of
    /// implemented again.
    pub reconciliation: Option<RoundIntegration>,
    pub reconciliation_state: Option<String>,
    pub final_status: Option<GoalStatus>,
    pub start_status: GoalStatus,
    git_remote: Option<String>,
}

impl<'a> WorkflowContext<'a> {
    pub(crate) fn revalidate_authority(&self, status: GoalStatus) -> RefineResult<()> {
        self.work_items.verify_workflow_attempt(
            &self.goal_id,
            self.attempt_authority,
            status,
            &self.node_id,
        )
    }

    // Context construction makes the runtime, target, Goal, node, provider, and Round axes
    // explicit so callers cannot derive execution authority from a local worker identity.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        runtime_root: &'a Path,
        target_root: &'a Path,
        goal_id: String,
        node_id: String,
        provider: String,
        round_idx: usize,
        attempt_authority: WorkflowAttemptAuthority,
        settings: JsonObject,
        work_items: FileWorkItemService,
    ) -> Self {
        Self {
            runtime_root,
            target_root,
            goal_id,
            node_id,
            provider,
            round_idx,
            attempt_authority,
            settings,
            work_items,
            branch: None,
            worktree_path: None,
            candidate_handoff_operation_id: None,
            quality_operation_id: None,
            quality_request: None,
            agent_cwd: None,
            provider_output: None,
            commit: None,
            implementation_changed: false,
            merge: None,
            reconciliation: None,
            reconciliation_state: None,
            final_status: None,
            start_status: GoalStatus::Plan,
            git_remote: None,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        self.runtime_root
    }

    pub fn target_root(&self) -> &Path {
        self.target_root
    }

    pub fn refine_dir(&self) -> PathBuf {
        self.work_items.refine_dir.clone()
    }

    pub fn request_transition(&mut self, from: GoalStatus, to: GoalStatus) -> RefineResult<()> {
        let current = self.work_items.show_goal_summary(&self.goal_id)?;
        let current_node = current.goal.node_id.as_deref().unwrap_or("default");
        if current_node != self.node_id {
            return Err(RefineError::Conflict(format!(
                "Goal {} moved from node {} to {} before workflow transition to {}",
                self.goal_id,
                self.node_id,
                current_node,
                to.as_str()
            )));
        }
        self.work_items.advance_claimed_goal_status(
            &self.goal_id,
            self.attempt_authority,
            from.clone(),
            to.clone(),
        )?;
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
            .entry("node_id".to_string())
            .or_insert_with(|| json!(&self.node_id));
        FileLogService::new(self.refine_dir()).append_round_log(
            &self.goal_id,
            self.round_idx,
            LogEntry {
                datetime: now_timestamp(),
                severity: "info".to_string(),
                category: category.to_string(),
                message: message.to_string(),
                details: Some(details),
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                goal_id: Some(self.goal_id.clone()),
            },
        )?;
        Ok(())
    }

    pub fn fail(&self, category: &str, error: &RefineError) -> RefineResult<()> {
        let _ = self.work_items.settle_workflow_attempt_failure(
            &self.goal_id,
            self.attempt_authority,
            category,
            &error.to_string(),
            &now_timestamp(),
        );
        self.log(
            category,
            &format!("Workflow failed: {error}"),
            Some(json_object(json!({"error": error.to_string()}))),
        )
    }

    pub fn workflow_process_metadata(&self, workflow_state: &str, behavior: &str) -> JsonObject {
        let mut metadata = workflow_subprocess_metadata(
            &self.goal_id,
            workflow_state,
            behavior,
            Some(self.round_idx),
        );
        metadata.insert("node_id".to_string(), json!(&self.node_id));
        metadata.insert("provider".to_string(), json!(&self.provider));
        metadata.insert(
            "workflow_revision".to_string(),
            json!(self.attempt_authority.workflow_revision),
        );
        metadata.insert(
            "target_app_id".to_string(),
            json!(self.target_root.display().to_string()),
        );
        metadata
    }

    pub fn require_branch(&self) -> RefineResult<&str> {
        self.branch
            .as_deref()
            .ok_or_else(|| missing_artifact("branch", &self.goal_id))
    }

    pub fn require_worktree_path(&self) -> RefineResult<&str> {
        self.worktree_path
            .as_deref()
            .ok_or_else(|| missing_artifact("worktree", &self.goal_id))
    }

    pub fn require_agent_cwd(&self) -> RefineResult<&Path> {
        self.agent_cwd
            .as_deref()
            .ok_or_else(|| missing_artifact("agent cwd", &self.goal_id))
    }

    pub fn require_commit(&self) -> RefineResult<&str> {
        self.commit
            .as_deref()
            .ok_or_else(|| missing_artifact("commit", &self.goal_id))
    }

    /// Returns the Git remote committed to this candidate round.
    ///
    /// Candidate publication and Governance integration must use one identity even if project
    /// settings change while the candidate is in flight.
    pub fn git_remote(&mut self) -> RefineResult<String> {
        if let Some(remote) = &self.git_remote {
            return Ok(remote.clone());
        }
        let detail = self.work_items.show_goal_detail(&self.goal_id)?;
        let round = detail
            .get("rounds")
            .and_then(serde_json::Value::as_array)
            .and_then(|rounds| rounds.get(self.round_idx))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Goal {} has no round {}",
                    self.goal_id,
                    self.round_idx + 1
                ))
            })?;
        if let Some(remote) = round
            .get("workflow_git_remote")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|remote| !remote.is_empty())
        {
            self.git_remote = Some(remote.to_string());
            return Ok(remote.to_string());
        }
        let remote =
            crate::application::workflow::setting_string(&self.settings, "git_remote", "origin");
        self.work_items.update_goal_round_evaluation_summary(
            &self.goal_id,
            self.round_idx,
            &json!({"workflow_git_remote": &remote}),
        )?;
        self.log(
            "git",
            "Committed Git remote for candidate integration",
            Some(crate::application::workflow::json_object(
                json!({"remote": &remote}),
            )),
        )?;
        self.git_remote = Some(remote.clone());
        Ok(remote)
    }
}

fn missing_artifact(name: &str, goal_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Goal {goal_id}"
    ))
}
