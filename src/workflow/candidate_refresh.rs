use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};

use super::context::WorkflowContext;

#[derive(Clone, Debug)]
pub(super) enum CandidateRefreshOutcome {
    Unchanged {
        /// The target tip the refresh observed, so integration can prove the
        /// target has not advanced between the refresh and the merge.
        target_commit: String,
    },
    Refreshed {
        original_candidate: String,
        replacement_candidate: String,
        target_commit: String,
        evidence: Value,
    },
    RecoveryQueued {
        reason: String,
        evidence: Value,
    },
    RecoveryExhausted {
        reason: String,
        evidence: Value,
    },
}

pub(super) fn refresh_candidate_for_target_advancement(
    ctx: &mut WorkflowContext<'_>,
    max_automatic_round_retries: u32,
) -> RefineResult<CandidateRefreshOutcome> {
    ctx.revalidate_authority(GoalStatus::Governance)?;
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let branch = required(&detail, "branch_name", &ctx.goal_id)?;
    let original_base = required(&detail, "base_commit", &ctx.goal_id)?;
    let original_candidate = required(&detail, "candidate_commit", &ctx.goal_id)?;
    let target_branch = required(&detail, "target_branch", &ctx.goal_id)?;
    let worktree = ctx.require_worktree_path()?.to_string();
    if ctx.require_branch()? != branch || ctx.require_commit()? != original_candidate {
        return Err(RefineError::Conflict(format!(
            "Goal {} hydrated candidate identity changed before integration lease",
            ctx.goal_id
        )));
    }

    let target_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
    let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree, ctx.runtime_root);
    let resolved_base = target_git.resolve_commit(&original_base);
    let resolved_candidate = target_git.resolve_commit(&original_candidate);
    let target_commit = target_git.resolve_commit(&target_branch)?;
    if resolved_base
        .as_ref()
        .is_ok_and(|resolved| resolved == &target_commit)
    {
        return Ok(CandidateRefreshOutcome::Unchanged { target_commit });
    }
    if resolved_candidate
        .as_ref()
        .is_ok_and(|resolved| resolved == &original_candidate)
        && target_git.commit_is_ancestor(&original_candidate, &target_commit)?
    {
        return Ok(CandidateRefreshOutcome::Unchanged { target_commit });
    }

    let head = worktree_git.head_ref()?;
    let status = worktree_git.inspect("")?;
    let provenance_error = if !resolved_base
        .as_ref()
        .is_ok_and(|resolved| resolved == &original_base)
    {
        Some("recorded base does not resolve to its exact durable commit")
    } else if !resolved_candidate
        .as_ref()
        .is_ok_and(|resolved| resolved == &original_candidate)
    {
        Some("recorded candidate does not resolve to its exact durable commit")
    } else if !target_git.commit_is_ancestor(&original_base, &original_candidate)? {
        Some("recorded base is not an ancestor of the exact candidate")
    } else if !target_git.commit_is_ancestor(&original_base, &target_commit)? {
        Some("advanced target does not descend from the recorded base")
    } else if !target_git.commit_range_is_linear(&original_base, &original_candidate)? {
        Some("base-to-candidate delta contains merge commits and has no implicit replay mainline")
    } else if head.branch.as_deref() != Some(branch.as_str())
        || head.commit.as_deref() != Some(original_candidate.as_str())
    {
        Some("clean candidate branch no longer names the exact recorded candidate")
    } else if status.dirty_user_changes || !status.refine_owned_artifacts.is_empty() {
        Some("candidate checkout is dirty")
    } else {
        None
    };
    if let Some(reason) = provenance_error {
        return queue_recovery(
            ctx,
            max_automatic_round_retries,
            reason,
            json!({
                "original_base_commit": original_base,
                "original_candidate_commit": original_candidate,
                "branch": branch,
                "worktree": worktree,
                "target_branch": target_branch,
                "target_commit": target_commit,
                "candidate_head": head,
                "candidate_status": status
            }),
        );
    }

    ctx.revalidate_authority(GoalStatus::Governance)?;
    let rebase = worktree_git.rebase(&target_branch)?;
    if !rebase.ok {
        let recovery = worktree_git.recover()?;
        let restored = worktree_git.head_ref()?;
        return queue_recovery(
            ctx,
            max_automatic_round_retries,
            "candidate refresh conflicted",
            json!({
                "original_base_commit": original_base,
                "original_candidate_commit": original_candidate,
                "branch": branch,
                "worktree": worktree,
                "target_branch": target_branch,
                "target_commit": target_commit,
                "rebase": rebase,
                "rebase_abort": recovery,
                "restored_candidate_head": restored
            }),
        );
    }
    let replacement = worktree_git.head_ref()?.commit.ok_or_else(|| {
        RefineError::Conflict(format!(
            "Goal {} refreshed candidate branch has no commit",
            ctx.goal_id
        ))
    })?;
    let refreshed_status = worktree_git.inspect("")?;
    if refreshed_status.dirty_user_changes || !refreshed_status.refine_owned_artifacts.is_empty() {
        return Err(RefineError::Conflict(format!(
            "Goal {} candidate refresh left a dirty worktree; replacement evidence was not persisted",
            ctx.goal_id
        )));
    }
    let evidence = persist_candidate_refresh_or_restore(
        ctx,
        &worktree_git,
        &branch,
        &worktree,
        &original_base,
        &original_candidate,
        &target_commit,
        &replacement,
    )?;
    ctx.commit = Some(replacement.clone());
    Ok(CandidateRefreshOutcome::Refreshed {
        original_candidate,
        replacement_candidate: replacement,
        target_commit,
        evidence,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn persist_candidate_refresh_or_restore(
    ctx: &mut WorkflowContext<'_>,
    worktree_git: &FileGitWorktreeService,
    branch: &str,
    worktree: &str,
    original_base: &str,
    original_candidate: &str,
    target_commit: &str,
    replacement: &str,
) -> RefineResult<Value> {
    let persist = (|| {
        ctx.revalidate_authority(GoalStatus::Governance)?;
        ctx.work_items.record_candidate_refresh(
            &ctx.goal_id,
            ctx.attempt_authority,
            &ctx.node_id,
            branch,
            worktree,
            original_base,
            original_candidate,
            target_commit,
            replacement,
        )
    })();
    match persist {
        Ok(evidence) => Ok(evidence),
        Err(error) => {
            match restore_unpersisted_refresh(worktree_git, branch, replacement, original_candidate)
            {
                Ok(()) => Err(error),
                Err(rollback) => Err(RefineError::Conflict(format!(
                    "{error}; candidate refresh replacement {replacement} was not persisted and branch {branch} could not be restored to {original_candidate}: {rollback}"
                ))),
            }
        }
    }
}

fn restore_unpersisted_refresh(
    worktree_git: &FileGitWorktreeService,
    branch: &str,
    replacement: &str,
    original_candidate: &str,
) -> RefineResult<()> {
    let head = worktree_git.head_ref()?;
    let status = worktree_git.inspect("")?;
    if head.branch.as_deref() != Some(branch)
        || head.commit.as_deref() != Some(replacement)
        || status.dirty_user_changes
        || !status.refine_owned_artifacts.is_empty()
    {
        return Err(RefineError::Conflict(
            "the refreshed checkout changed or became dirty after rebase; user work was preserved"
                .to_string(),
        ));
    }
    worktree_git.reset_hard_to(original_candidate)
}

fn queue_recovery(
    ctx: &mut WorkflowContext<'_>,
    max_automatic_round_retries: u32,
    reason: &str,
    evidence: Value,
) -> RefineResult<CandidateRefreshOutcome> {
    let recovery = ctx.work_items.queue_integration_recovery_summary(
        &ctx.goal_id,
        ctx.attempt_authority,
        &ctx.node_id,
        reason,
        evidence.clone(),
        max_automatic_round_retries,
    )?;
    if recovery.goal.status == GoalStatus::Todo {
        Ok(CandidateRefreshOutcome::RecoveryQueued {
            reason: reason.to_string(),
            evidence,
        })
    } else {
        Ok(CandidateRefreshOutcome::RecoveryExhausted {
            reason: reason.to_string(),
            evidence,
        })
    }
}

fn required(value: &Value, key: &str, goal_id: &str) -> RefineResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no recorded {key}")))
}
