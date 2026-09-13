//! One lineage decision for new, resumed and directly selected workflow steps.
use super::*;
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, validate_branch_name};
use serde_json::Value;

pub(crate) struct PreparedWorkspace {
    pub branch: String,
    pub base: String,
    pub candidate: Option<String>,
    pub status: GoalStatus,
}

/// Select only a receipt describing the current candidate and target. Actual
/// repository effects remain the integration resolver's responsibility.
pub(crate) fn matching_round_integration(
    goal: &Value,
    round_idx: usize,
    target: &str,
) -> Option<crate::model::goal::RoundIntegration> {
    if !crate::application::workflow::governance::integration::recorded_integration_is_applicable(
        &goal["rounds"][round_idx],
    ) {
        return None;
    }
    let integration = serde_json::from_value::<crate::model::goal::RoundIntegration>(
        goal["rounds"][round_idx]["workflow_integration"].clone(),
    )
    .ok()?;
    (goal["candidate_commit"].as_str() == Some(integration.candidate_commit.as_str())
        && integration.target_branch == target
        && goal["target_branch"].as_str() == Some(target))
    .then_some(integration)
}

pub(crate) fn prepare_workspace(
    ctx: &mut WorkflowContext<'_>,
    status: GoalStatus,
    target: &str,
) -> RefineResult<PreparedWorkspace> {
    ctx.revalidate_authority(status.clone())?;
    crate::application::workflow::engine::behaviors::refresh_workflow_target_from_remote(
        ctx,
        &FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root),
        target,
    )?;
    let repository = ctx.target_root.to_path_buf();
    with_repository_git_lock(&repository, || {
        ctx.revalidate_authority(status.clone())?;
        let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
        let git = FileGitWorktreeService::with_runtime_root(&repository, ctx.runtime_root);
        let expected_branch = ctx.round_branch()?;
        let text = |key: &str| goal[key].as_str().map(str::trim).filter(|s| !s.is_empty());
        let recorded_branch = text("branch_name");
        let base = text("base_commit");
        let candidate = text("candidate_commit");
        let resolved = |value: Option<&str>| -> RefineResult<Option<String>> {
            match value {
                Some(value) => match git.find_commit(value) {
                    Err(RefineError::InvalidInput(_)) => Ok(None),
                    result => result,
                },
                None => Ok(None),
            }
        };
        let base_commit = resolved(base)?;
        let candidate_commit = resolved(candidate)?;
        // A persisted alias is generated input. Reject its syntax before Git
        // observation, then let the shared recovery path choose a fresh binding.
        let branch_valid = match validate_branch_name(&expected_branch) {
            Ok(()) => true,
            Err(RefineError::InvalidInput(_)) => false,
            Err(error) => return Err(error),
        };
        let branch_tip = if branch_valid {
            resolved(Some(&expected_branch))?
        } else {
            None
        };
        let workspace_usable = branch_valid
            && !git.worktree_requires_reconstruction(
                &expected_branch,
                matches!(status, GoalStatus::Quality | GoalStatus::Governance),
            )?;
        let fresh = recorded_branch.is_none()
            && base.is_none()
            && candidate.is_none()
            && branch_tip.is_none();
        let lineage_matches = if let Some(base) = &base_commit {
            let candidate_ok = match &candidate_commit {
                Some(commit) => git.commit_is_ancestor(base, commit)?,
                None => candidate.is_none(),
            };
            let branch_ok = match &branch_tip {
                Some(commit) => git.commit_is_ancestor(base, commit)?,
                None => true,
            };
            let candidate_on_branch = match (&candidate_commit, &branch_tip) {
                (Some(candidate), Some(tip))
                    if matches!(status, GoalStatus::Quality | GoalStatus::Governance) =>
                {
                    candidate == tip
                }
                (Some(candidate), Some(tip)) => git.commit_is_ancestor(candidate, tip)?,
                _ => true,
            };
            candidate_on_branch
                && base.as_str() == text("base_commit").unwrap_or_default()
                && candidate_ok
                && branch_ok
                && candidate_commit.as_deref() == candidate
        } else {
            false
        };
        let executable_candidate = !matches!(status, GoalStatus::Quality | GoalStatus::Governance)
            || candidate_commit.is_some();
        let round = &goal["rounds"][ctx.round_idx];
        let integration_consistent = round["workflow_integration"].is_null()
            || matching_round_integration(&goal, ctx.round_idx, target).is_some();
        let retained_seed =
            candidate.is_some() && round["retained_candidate"].as_str() == candidate;
        let legacy_seed = matches!(
            round["automatic_retry"]["kind"].as_str(),
            Some("quality" | "governance")
        ) && round["automatic_retry"]["source_round"]
            .as_u64()
            .is_some_and(|source| source > 0 && source <= ctx.round_idx as u64);
        let seed = if (retained_seed || legacy_seed) && branch_tip.is_none() {
            match (&base_commit, &candidate_commit) {
                (Some(base), Some(candidate)) => git.commit_is_ancestor(base, candidate)?,
                _ => false,
            }
        } else {
            false
        };
        let compatible = integration_consistent
            && workspace_usable
            && recorded_branch == Some(expected_branch.as_str())
            && text("target_branch") == Some(target)
            && lineage_matches
            && executable_candidate;
        let mut recovery = None;
        let (branch, base, candidate) = if compatible
            || (seed && workspace_usable && integration_consistent)
        {
            (expected_branch, base_commit.unwrap(), candidate_commit)
        } else if fresh
            && workspace_usable
            && integration_consistent
            && matches!(
                status,
                GoalStatus::Todo | GoalStatus::Plan | GoalStatus::Implement
            )
        {
            (expected_branch, git.resolve_commit(target)?, None)
        } else {
            recovery = Some(
                "Generated workspace inputs are missing or inconsistent; reproduce the authored Round in a clean execution",
            );
            // A new binding is durable before Git creates anything. It survives
            // interruption and never resets a previous execution's branch.
            let branch = format!(
                "{}-execution-{}",
                execution::implementation_branch_name(
                    &crate::application::workflow::setting_string(
                        &ctx.settings,
                        "branch_name_pattern",
                        "refine/{goal_id}"
                    ),
                    &ctx.goal_id,
                    ctx.round_idx
                ),
                uuid::Uuid::new_v4()
            );
            // Configuration is authored input: an invalid replacement pattern
            // must remain visible instead of becoming a new broken binding.
            validate_branch_name(&branch)?;
            (branch, git.resolve_commit(target)?, None)
        };
        let observed = ctx.work_items.bind_prepared_workspace(
            &ctx.goal_id,
            ctx.attempt_authority,
            &status,
            &branch,
            target,
            &base,
            candidate.as_deref(),
            recovery,
        )?;
        ctx.attempt_authority = observed;
        ctx.work_items
            .bind_workflow_occurrence(&ctx.goal_id, observed);
        let status = if recovery.is_some() {
            GoalStatus::Plan
        } else {
            status
        };
        ctx.start_status = status.clone();
        Ok(PreparedWorkspace {
            branch,
            base,
            candidate,
            status,
        })
    })
}

#[cfg(test)]
mod tests;
