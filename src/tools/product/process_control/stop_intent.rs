use super::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum GoalStopDisposition {
    Cancel,
    Requeue,
}

impl GoalStopDisposition {
    pub(super) fn goal_status(self) -> GoalStatus {
        match self {
            Self::Cancel => GoalStatus::Cancelled,
            Self::Requeue => GoalStatus::Todo,
        }
    }
}

pub(super) fn default_goal_stop_disposition() -> GoalStopDisposition {
    GoalStopDisposition::Cancel
}

/// Authoritative product intent for a process termination.
///
/// Callers must select the intent before process discovery or claim inspection. Those are
/// settlement inputs and evidence, not a source from which terminal Goal semantics may be
/// inferred.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminationIntent {
    ExplicitCancellation,
    InteractiveStop,
}

impl TerminationIntent {
    pub(super) fn disposition(self) -> GoalStopDisposition {
        match self {
            Self::ExplicitCancellation => GoalStopDisposition::Cancel,
            Self::InteractiveStop => GoalStopDisposition::Requeue,
        }
    }

    pub(super) fn expected_goal_status(self) -> GoalStatus {
        self.disposition().goal_status()
    }

    pub(super) fn from_legacy_disposition(disposition: GoalStopDisposition) -> Self {
        match disposition {
            GoalStopDisposition::Cancel => Self::ExplicitCancellation,
            GoalStopDisposition::Requeue => Self::InteractiveStop,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct WorkflowWorktree {
    pub(super) path: PathBuf,
    pub(super) branch: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct WorkflowWorktreeRetention {
    pub(super) retained: bool,
    pub(super) worktrees: Vec<WorkflowWorktree>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) recovery: Option<String>,
}

impl WorkflowWorktreeRetention {
    pub(super) fn from_targets(targets: &[WorkflowWorktree]) -> Self {
        let mut worktrees = Vec::new();
        for target in targets {
            if !worktrees.contains(target) {
                worktrees.push(target.clone());
            }
        }
        let retained = !worktrees.is_empty();
        Self {
            retained,
            worktrees,
            recovery: retained.then(|| {
                "all workflow worktrees and branches were retained; inspect, commit, or preserve agent work before using a separate explicit human-controlled cleanup operation".to_string()
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct GoalStopSettlement {
    pub(super) goal: crate::tools::product::project_state::GoalSummaryProjection,
    pub(super) worktree_retention: WorkflowWorktreeRetention,
}

pub(super) fn workflow_worktree(
    process: &ManagedProcess,
) -> RefineResult<Option<WorkflowWorktree>> {
    let metadata = process_metadata(process);
    let Some(worktree) = metadata.get("worktree") else {
        return Ok(None);
    };
    let path = worktree
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "managed process {} has incomplete worktree ownership: path is required; termination was not requested",
                process.id
            ))
        })?;
    let branch = worktree
        .get("branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "managed process {} has incomplete worktree ownership: branch is required; termination was not requested",
                process.id
            ))
        })?;
    let path = PathBuf::from(path);
    let canonical_path = path.canonicalize().map_err(|error| {
        RefineError::Conflict(format!(
            "managed process {} worktree {} cannot be resolved: {error}; termination was not requested",
            process.id,
            path.display()
        ))
    })?;
    if let Some(cwd) = metadata.get("cwd").and_then(Value::as_str) {
        let cwd = Path::new(cwd).canonicalize().map_err(|error| {
            RefineError::Conflict(format!(
                "managed process {} cwd {cwd} cannot be resolved: {error}; termination was not requested",
                process.id
            ))
        })?;
        if !cwd.starts_with(&canonical_path) {
            return Err(RefineError::Conflict(format!(
                "managed process {} cwd {} is outside its recorded worktree {}; termination was not requested",
                process.id,
                cwd.display(),
                canonical_path.display()
            )));
        }
    }
    Ok(Some(WorkflowWorktree {
        path: canonical_path,
        branch: branch.to_string(),
    }))
}
