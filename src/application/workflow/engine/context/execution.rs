use serde_json::{Value, json};

use crate::application::work_items::FileWorkItemService;
use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::recovery::candidate_handoff::{
    find_candidate_handoff, register_candidate_handoff,
};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::model::workflow::GoalStatus;

use crate::application::workflow::json_object;

pub(crate) fn authored_workflow_commitment(
    work_items: &FileWorkItemService,
    goal_id: &str,
) -> RefineResult<(usize, u64, String)> {
    work_items.authored_goal_commitment(goal_id)
}

pub(crate) fn hydrate_retry_context(
    ctx: &mut WorkflowContext<'_>,
    current: GoalStatus,
) -> RefineResult<()> {
    let target =
        crate::application::workflow::setting_string(&ctx.settings, "merge_target_branch", "main");
    let prepared = super::preparation::prepare_workspace(ctx, current.clone(), &target)?;
    if prepared.status == GoalStatus::Plan {
        let pattern = crate::application::workflow::setting_string(
            &ctx.settings,
            "branch_name_pattern",
            "refine/{goal_id}",
        );
        return materialize_prepared_workspace(ctx, &pattern, prepared);
    }
    ctx.revalidate_authority(current.clone())?;
    ctx.start_status = current.clone();
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
    ctx.branch = Some(branch.clone());
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
    let integration =
        super::preparation::matching_round_integration(&detail, ctx.round_idx, &target);
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
    } else if current == GoalStatus::Quality
        && let Some(integration) = integration.clone()
    {
        // A current-Round integration identity is the admission signal for the shared terminal
        // resolver. Do not inspect the moving shared target here or rerun Quality against it.
        ctx.reconciliation = Some(integration);
        ctx.reconciliation_state = Some("pending".to_string());
    } else if reconciliation_state.is_none()
        && let Some(integration) = integration.clone()
    {
        let app_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
        let detected = with_repository_git_lock(ctx.target_root, || {
            ctx.revalidate_authority(current.clone())?;
            let target_commit = app_git.resolve_commit(&integration.target_branch)?;
            if !app_git.commit_is_ancestor(&integration.candidate_commit, &target_commit)? {
                return Ok(None);
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
                        "detected_at": crate::application::workflow::now_timestamp()
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
    if ctx.reconciliation_state.is_none() || current == GoalStatus::Governance {
        super::validate_round_workspace_branch(
            &detail,
            &ctx.goal_id,
            ctx.round_idx,
            &branch,
            &crate::application::workflow::setting_string(
                &ctx.settings,
                "branch_name_pattern",
                "refine/{goal_id}",
            ),
        )?;
        ensure_resumed_candidate_worktree(ctx, &branch, &candidate, &base)?;
    } else {
        // The Quality-reconciliation lane materializes its own exact-candidate checkout
        // inside the quality runner; a stale worktree registration must not surface as a
        // phantom path here.
        let worktree = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root)
            .existing_worktree_for_branch(&branch)?
            .filter(|path| path.exists());
        let worktree = worktree.filter(|path| {
            crate::infrastructure::git::worktrees::ManagedWorktree {
                repository: ctx.target_root.to_path_buf(),
                path: path.clone(),
                branch: branch.clone(),
                commit: None,
                allow_rebase: false,
                registration: None,
            }
            .validate()
            .is_ok()
        });
        ctx.worktree_path = worktree.as_ref().map(|path| path.display().to_string());
        ctx.agent_cwd = worktree
            .map(|path| {
                agent_worktree_cwd(
                    &path.to_string_lossy(),
                    &crate::application::workflow::setting_string(
                        &ctx.settings,
                        "agent_subpath",
                        "",
                    ),
                )
            })
            .transpose()?;
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

/// Rebuilds the candidate worktree for a resumed Goal when external cleanup removed it.
///
/// The Round branch and recorded candidate commit are the durable contract; the worktree
/// directory is disposable scratch space. Resume therefore re-materializes the checkout the
/// same way `hydrate_plan_or_implement_context` does instead of failing terminally on the
/// missing directory, and fails closed only when the branch no longer contains the recorded
/// candidate.
fn ensure_resumed_candidate_worktree(
    ctx: &mut WorkflowContext<'_>,
    branch: &str,
    candidate: &str,
    base: &str,
) -> RefineResult<()> {
    let git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
    // Goal preparation selected this exact workspace and base. Historical handoff
    // receipts describe older execution; they cannot select the current inputs.
    let target = git.managed_worktree_path(branch)?;
    let (worktree, handoff) = with_repository_git_lock(ctx.target_root, || {
        ctx.revalidate_authority(ctx.start_status.clone())?;
        let worktree = match git.find_commit(&format!("refs/heads/{branch}"))? {
            Some(tip) => {
                if tip != candidate && !matches!(git.commit_is_ancestor(candidate, &tip), Ok(true))
                {
                    return Err(RefineError::Conflict(format!(
                        "Goal {} cannot resume: branch {branch} names {tip}, which does not contain recorded candidate {candidate}; existing work was preserved",
                        ctx.goal_id
                    )));
                }
                if target.is_dir()
                    && FileGitWorktreeService::new(&target).operation_in_progress()?
                {
                    // A retained rebase has detached HEAD. Prove its recorded branch
                    // before allowing the existing refresh recovery to inspect it.
                    crate::infrastructure::git::worktrees::ManagedWorktree {
                        repository: ctx.target_root.into(),
                        path: target.clone(),
                        branch: branch.into(),
                        commit: None,
                        allow_rebase: true,
                        registration: None,
                    }
                    .pin()?;
                    target.display().to_string()
                } else {
                    git.ensure_worktree(branch, &target)?
                }
            }
            // The branch ref is gone (external cleanup): pin the exact recorded candidate
            // without moving any other ref.
            None => git.ensure_worktree_at_commit(branch, &target, candidate)?,
        };
        let handoff = register_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
            &ctx.node_id,
            branch,
            &worktree,
            base,
        )?;
        Ok((worktree, handoff))
    })?;
    ctx.worktree_path = Some(worktree.clone());
    ctx.candidate_handoff_operation_id = Some(handoff.id);
    ctx.agent_cwd = Some(agent_worktree_cwd(
        &worktree,
        &crate::application::workflow::setting_string(&ctx.settings, "agent_subpath", ""),
    )?);
    ctx.log(
        "workflow",
        "Ensured candidate worktree while resuming",
        Some(json_object(json!({
            "branch": branch,
            "worktree": worktree,
            "candidate_commit": candidate
        }))),
    )
}

/// Restores the implementation workspace for an interrupted Plan or Implement Goal.
///
/// Plan or Implement may have stopped anywhere between the durable status transition and the first
/// candidate commit. The branch name is deterministic, completed planning artifacts live on the
/// Round, and `ensure_worktree_from_base` is idempotent, so restarting is cheaper and safer than
/// persisting a worker identity.
pub(crate) fn hydrate_plan_or_implement_context(
    ctx: &mut WorkflowContext<'_>,
    branch_pattern: &str,
    target_branch: &str,
) -> RefineResult<()> {
    let observed_status = ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status;
    if !matches!(observed_status, GoalStatus::Plan | GoalStatus::Implement) {
        return Err(RefineError::Conflict(
            "Current workflow step does not authorize an implementation workspace".into(),
        ));
    }
    let prepared = super::preparation::prepare_workspace(ctx, observed_status, target_branch)?;
    materialize_prepared_workspace(ctx, branch_pattern, prepared)
}

pub(crate) fn materialize_prepared_workspace(
    ctx: &mut WorkflowContext<'_>,
    branch_pattern: &str,
    prepared: super::preparation::PreparedWorkspace,
) -> RefineResult<()> {
    let branch = prepared.branch;
    let base = prepared.base;
    let git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    super::validate_round_workspace_branch(
        &detail,
        &ctx.goal_id,
        ctx.round_idx,
        &branch,
        branch_pattern,
    )?;
    let worktree_target = git.managed_worktree_path(&branch)?;
    let (worktree, handoff) = with_repository_git_lock(ctx.target_root, || {
        ctx.revalidate_authority(ctx.start_status.clone())?;
        let worktree = if let Some(candidate) = prepared.candidate.as_deref()
            && git.find_commit(&format!("refs/heads/{branch}"))?.is_none()
        {
            git.ensure_worktree_at_commit(&branch, &worktree_target, candidate)?
        } else {
            git.ensure_worktree_from_base(&branch, &worktree_target, &base)?
        };
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
    ctx.branch = Some(branch.clone());
    ctx.worktree_path = Some(worktree.clone());
    ctx.candidate_handoff_operation_id = Some(handoff.id);
    ctx.agent_cwd = Some(agent_worktree_cwd(
        &worktree,
        &crate::application::workflow::setting_string(&ctx.settings, "agent_subpath", ""),
    )?);
    ctx.log(
        "workflow",
        "Prepared implementation worktree from coherent Round inputs",
        Some(json_object(json!({
            "status": ctx.start_status.as_str(),
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

pub(crate) fn implementation_branch_name(pattern: &str, goal_id: &str, round_idx: usize) -> String {
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

pub(crate) use crate::infrastructure::git::worktrees::agent_worktree_cwd;
