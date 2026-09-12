use super::*;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, ManagedWorktree};
use serde_json::Value;

impl WorkflowContext<'_> {
    pub(crate) fn round_branch(&self) -> RefineResult<String> {
        let detail = self.work_items.show_goal_detail(&self.goal_id)?;
        Ok(detail["rounds"][self.round_idx]["workspace_branch"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                execution::implementation_branch_name(
                    &crate::application::workflow::setting_string(
                        &self.settings,
                        "branch_name_pattern",
                        "refine/{goal_id}",
                    ),
                    &self.goal_id,
                    self.round_idx,
                )
            }))
    }

    pub(crate) fn managed_worktree(&self) -> RefineResult<ManagedWorktree> {
        self.workspace_for_mode(false)
    }

    pub(crate) fn refresh_workspace(&self) -> RefineResult<ManagedWorktree> {
        self.workspace_for_mode(true)
    }

    fn workspace_for_mode(&self, allow_rebase: bool) -> RefineResult<ManagedWorktree> {
        let summary = self.work_items.show_goal_summary(&self.goal_id)?;
        if summary.goal.round_count.checked_sub(1) != Some(self.round_idx)
            || summary.goal.branch_name.as_deref() != Some(self.require_branch()?)
        {
            return Err(crate::error::RefineError::Degraded(
                "Goal Round or branch ownership changed; retained work was preserved".into(),
            ));
        }
        validate_round_workspace_branch(
            &self.work_items.show_goal_detail(&self.goal_id)?,
            &self.goal_id,
            self.round_idx,
            self.require_branch()?,
            &crate::application::workflow::setting_string(
                &self.settings,
                "branch_name_pattern",
                "refine/{goal_id}",
            ),
        )?;
        let workspace = ManagedWorktree {
            repository: self.target_root.to_path_buf(),
            path: PathBuf::from(
                self.worktree_path
                    .as_deref()
                    .ok_or_else(|| missing_artifact("worktree", &self.goal_id))?,
            ),
            branch: self.require_branch()?.to_string(),
            commit: None,
            allow_rebase,
            registration: None,
        };
        let mut admitted = self.admitted_worktree.borrow_mut();
        if let Some(previous) = admitted.as_ref() {
            if previous.repository != workspace.repository
                || previous.path != workspace.path
                || previous.branch != workspace.branch
            {
                return Err(crate::error::RefineError::Degraded(
                    "Goal workspace identity changed; retained work was preserved".into(),
                ));
            }
            let mut workspace = previous.clone();
            workspace.allow_rebase = allow_rebase;
            workspace.validate()?;
            return Ok(workspace);
        }
        let workspace = workspace.pin()?;
        *admitted = Some(workspace.clone());
        Ok(workspace)
    }

    pub(crate) fn refresh_git(&self) -> RefineResult<FileGitWorktreeService> {
        let workspace = self.refresh_workspace()?;
        FileGitWorktreeService::with_runtime_root(&workspace.path, self.runtime_root)
            .with_managed_worktree(workspace)
    }

    pub(crate) fn candidate_git(&self) -> RefineResult<FileGitWorktreeService> {
        let workspace = self.managed_worktree()?;
        FileGitWorktreeService::with_runtime_root(&workspace.path, self.runtime_root)
            .with_managed_worktree(workspace)
    }
}

/// A top-level Goal branch can still name a previous Round after an interrupted
/// admission. Prefer the current Round's durable plan binding; before planning,
/// require the branch derived for this Goal and Round by the configured pattern.
pub(crate) fn validate_round_workspace_branch(
    detail: &Value,
    goal_id: &str,
    round_idx: usize,
    branch: &str,
    branch_pattern: &str,
) -> RefineResult<()> {
    let binding = &detail["rounds"][round_idx]["implementation_plan"]["binding"];
    let expected = if let Some(branch) = detail["rounds"][round_idx]["workspace_branch"].as_str() {
        branch.to_string()
    } else if binding.is_object() {
        if binding["goal_id"].as_str() != Some(goal_id)
            || binding["round_idx"].as_u64() != Some(round_idx as u64)
        {
            return Err(RefineError::Degraded("Round workspace plan binding has different Goal or Round ownership; existing work was preserved".into()));
        }
        binding["implementation_branch"]
            .as_str()
            .ok_or_else(|| {
                RefineError::Degraded(
                    "Round workspace plan binding has no branch; existing work was preserved"
                        .into(),
                )
            })?
            .to_string()
    } else {
        if !branch_pattern.trim().is_empty()
            && !branch_pattern.contains("{goal_id}")
            && !branch_pattern.contains("{goal}")
        {
            return Err(RefineError::Degraded("Workspace branch pattern must include the Goal identity until a Round plan binds its branch; existing work was preserved".into()));
        }
        execution::implementation_branch_name(branch_pattern, goal_id, round_idx)
    };
    if branch != expected {
        return Err(RefineError::Degraded(format!(
            "Round workspace branch {branch} does not belong to Goal {goal_id} Round {}; expected {expected}; existing work was preserved",
            round_idx + 1
        )));
    }
    Ok(())
}
