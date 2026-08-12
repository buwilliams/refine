use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::model::goal::RoundIntegration;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::candidate_handoff::{find_candidate_handoff, register_candidate_handoff};
use crate::workflow::context::WorkflowContext;

use super::json_object;

pub(super) fn authored_workflow_commitment(
    work_items: &FileWorkItemService,
    goal_id: &str,
) -> RefineResult<(usize, u64, String)> {
    work_items.authored_goal_commitment(goal_id)
}

pub(super) fn hydrate_retry_context(
    ctx: &mut WorkflowContext<'_>,
    current: GoalStatus,
) -> RefineResult<()> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let branch = required_workflow_string(&detail, "branch_name", &ctx.goal_id)?;
    let candidate = required_workflow_string(&detail, "candidate_commit", &ctx.goal_id)?;
    let base = required_workflow_string(&detail, "base_commit", &ctx.goal_id)?;
    let round = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} has no round {} to resume",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    let worktree = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root)
        .existing_worktree_for_branch(&branch)?;
    ctx.branch = Some(branch.clone());
    ctx.worktree_path = worktree.as_ref().map(|path| path.display().to_string());
    ctx.agent_cwd = worktree;
    ctx.provider_output = Some(
        round
            .get("implementation_report")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "Resumed existing workflow candidate".to_string()),
    );
    ctx.commit = Some(candidate.clone());
    ctx.candidate_handoff_operation_id = find_candidate_handoff(
        ctx.runtime_root,
        ctx.target_root,
        &ctx.goal_id,
        ctx.round_idx,
    )?
    .map(|operation| operation.id);
    ctx.implementation_changed = candidate != base;
    let integration = round
        .get("workflow_integration")
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value::<RoundIntegration>(value.clone()).map_err(|error| {
                RefineError::Serialization(format!(
                    "Goal {} has invalid Ready Merge evidence: {error}",
                    ctx.goal_id
                ))
            })
        })
        .transpose()?;
    ctx.merge = integration
        .as_ref()
        .map(|integration| integration.merge.clone());
    let reconciliation_state = round
        .get("workflow_reconciliation")
        .and_then(Value::as_object)
        .and_then(|evidence| evidence.get("state"))
        .and_then(Value::as_str);
    if let Some(state) =
        reconciliation_state.filter(|state| matches!(*state, "detected" | "revert_blocked"))
    {
        ctx.reconciliation = integration.clone();
        ctx.reconciliation_state = Some(state.to_string());
    } else if reconciliation_state.is_none()
        && let Some(integration) = integration.clone()
    {
        let app_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
        let detected = with_repository_git_lock(ctx.target_root, || {
            let target_commit = app_git.resolve_commit(&integration.target_branch)?;
            if !app_git.commit_is_ancestor(&integration.candidate_commit, &target_commit)? {
                return Ok(None);
            }
            let head = app_git.head_ref()?;
            if head.branch.as_deref() != Some(integration.target_branch.as_str())
                || head.commit.as_deref() != Some(target_commit.as_str())
            {
                return Err(RefineError::Conflict(format!(
                    "Goal {} already-merged reconciliation requires target worktree {} at {} on branch {}, found {} at {}; user work was preserved",
                    ctx.goal_id,
                    ctx.target_root.display(),
                    target_commit,
                    integration.target_branch,
                    head.branch.as_deref().unwrap_or("<detached>"),
                    head.commit.as_deref().unwrap_or("<unborn>")
                )));
            }
            let published = if integration.pushed {
                app_git.fetch_branch(&integration.remote, &integration.target_branch)?;
                let published = app_git.resolve_commit(&format!(
                    "{}/{}",
                    integration.remote, integration.target_branch
                ))?;
                if published != target_commit {
                    return Err(RefineError::Conflict(format!(
                        "Goal {} cannot reconcile while local {} ({target_commit}) differs from {}/{} ({published})",
                        ctx.goal_id,
                        integration.target_branch,
                        integration.remote,
                        integration.target_branch
                    )));
                }
                Some(published)
            } else {
                None
            };
            Ok(Some((target_commit, published)))
        })?;
        if let Some((target_commit, published_commit)) = detected {
            ctx.work_items.update_goal_round_evaluation_summary(
                &ctx.goal_id,
                ctx.round_idx,
                &json!({
                    "workflow_reconciliation": {
                        "state": "detected",
                        "candidate_commit": integration.candidate_commit,
                        "target_branch": integration.target_branch,
                        "detected_target_commit": target_commit,
                        "published_target_commit": published_commit,
                        "detected_at": crate::workflow::now_timestamp()
                    }
                }),
            )?;
            ctx.log(
                "reconcile",
                "Detected already-merged candidate while resuming workflow; routing Quality to the merged target",
                Some(json_object(json!({
                    "resumed_status": current.as_str(),
                    "candidate_commit": integration.candidate_commit,
                    "target_branch": integration.target_branch,
                    "target_commit": target_commit,
                    "published_target_commit": published_commit
                }))),
            )?;
            ctx.reconciliation = Some(integration);
            ctx.reconciliation_state = Some("detected".to_string());
        }
    }
    ctx.start_status = current.clone();
    ctx.log(
        "workflow",
        &format!("Resumed workflow from {}", current.as_str()),
        Some(json_object(json!({
            "status": current.as_str(),
            "branch": branch,
            "candidate_commit": candidate,
            "round": ctx.round_idx + 1
        }))),
    )
}

/// Restores the implementation workspace for an interrupted in-progress Goal.
///
/// In-progress may have stopped anywhere between the durable status transition and the first
/// candidate commit. The branch name is deterministic, completed planning artifacts live on the
/// Round, and `ensure_worktree` is idempotent, so restarting is cheaper and safer than persisting
/// a worker identity.
pub(super) fn hydrate_in_progress_context(
    ctx: &mut WorkflowContext<'_>,
    branch_pattern: &str,
    target_branch: &str,
) -> RefineResult<()> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let branch = detail
        .get("branch_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| implementation_branch_name(branch_pattern, &ctx.goal_id, ctx.round_idx));
    let git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
    let base = detail
        .get("base_commit")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or(git.resolve_commit(target_branch)?);
    let worktree_target = git
        .git_path("refine-worktrees")?
        .join(branch.replace('/', "-"));
    let (worktree, handoff) = with_repository_git_lock(ctx.target_root, || {
        let worktree = git.ensure_worktree(&branch, &worktree_target)?;
        let handoff = register_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
            &ctx.node_id,
            &branch,
            &worktree,
            &base,
        )?;
        Ok((worktree, handoff))
    })?;
    ctx.work_items.update_goal_git_refs(
        &ctx.goal_id,
        &branch,
        target_branch,
        &base,
        detail
            .get("candidate_commit")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )?;
    ctx.branch = Some(branch.clone());
    ctx.worktree_path = Some(worktree.clone());
    ctx.candidate_handoff_operation_id = Some(handoff.id);
    ctx.agent_cwd = Some(PathBuf::from(&worktree));
    ctx.start_status = GoalStatus::InProgress;
    ctx.log(
        "workflow",
        "Restarted interrupted implementation from its retained worktree",
        Some(json_object(json!({
            "status": GoalStatus::InProgress.as_str(),
            "branch": branch,
            "worktree": worktree,
            "round": ctx.round_idx + 1
        }))),
    )
}

fn required_workflow_string(value: &Value, key: &str, goal_id: &str) -> RefineResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no recorded {key} to resume workflow"
            ))
        })
}

pub(super) fn implementation_branch_name(pattern: &str, goal_id: &str, round_idx: usize) -> String {
    let pattern = pattern.trim();
    let base = if pattern.is_empty() {
        "refine/{goal_id}"
    } else {
        pattern
    };
    let round = (round_idx + 1).to_string();
    let branch = base
        .replace("{goal_id}", goal_id)
        .replace("{goal}", goal_id)
        .replace("{round}", &round);
    if branch.contains(&format!("round-{round}")) || branch.contains(&format!("round/{round}")) {
        branch
    } else {
        format!("{branch}/round-{round}")
    }
}

pub(super) fn agent_worktree_cwd(
    worktree_path: &str,
    agent_subpath: &str,
) -> RefineResult<PathBuf> {
    let root = PathBuf::from(worktree_path);
    let subpath = agent_subpath.trim();
    if subpath.is_empty() {
        return Ok(root);
    }
    let relative = Path::new(subpath);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(RefineError::InvalidInput(
            "agent_subpath must be a relative path inside the worktree".to_string(),
        ));
    }
    Ok(root.join(relative))
}
