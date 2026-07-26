use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::model::goal::RoundIntegration;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::FileGitWorktreeService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::context::WorkflowContext;

use super::json_object;

pub(super) fn ensure_workflow_round(
    work_items: &FileWorkItemService,
    goal_id: &str,
) -> RefineResult<usize> {
    let goal = work_items.show_goal_summary(goal_id)?;
    if let Some(idx) = goal.goal.round_count.checked_sub(1) {
        return Ok(idx);
    }
    let goal = work_items.append_goal_round_summary(
        goal_id,
        "Refine",
        "Implement and verify this Goal.",
    )?;
    goal.goal
        .round_count
        .checked_sub(1)
        .ok_or_else(|| RefineError::InvalidInput(format!("Goal {goal_id} has no rounds")))
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
    ctx.implementation_changed = candidate != base;
    ctx.merge = round
        .get("workflow_integration")
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value::<RoundIntegration>(value.clone())
                .map(|integration| integration.merge)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "Goal {} has invalid Ready Merge evidence: {error}",
                        ctx.goal_id
                    ))
                })
        })
        .transpose()?;
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
