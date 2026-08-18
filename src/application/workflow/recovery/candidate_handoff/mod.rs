use std::path::Path;

use serde_json::{Value, json};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};

const HANDOFF_KIND: &str = "workflow_candidate_handoff";

#[allow(clippy::too_many_arguments)]
pub(crate) fn register_candidate_handoff(
    runtime_root: &Path,
    target_root: &Path,
    goal_id: &str,
    round_idx: usize,
    node_id: &str,
    branch: &str,
    worktree_path: &str,
    base_commit: &str,
) -> RefineResult<OperationHandle> {
    let registry = FileOperationRegistry::new(runtime_root);
    let owner = handoff_owner(target_root, goal_id, round_idx);
    if let Some(operation) = active_handoff(&registry, &owner)? {
        validate_handoff(
            &operation,
            goal_id,
            round_idx,
            branch,
            worktree_path,
            base_commit,
        )?;
        settle_superseded_handoffs(&registry, target_root, goal_id, round_idx, &operation.id)?;
        return Ok(operation);
    }
    let operation = registry.register_exclusive_with_request(
        &owner,
        json!({
            "kind": HANDOFF_KIND,
            "goal_id": goal_id,
            "round_idx": round_idx,
            "node_id": node_id,
            "target_app_id": target_root.display().to_string(),
            "target_root": target_root.display().to_string(),
            "branch": branch,
            "path": worktree_path,
            "worktree_path": worktree_path,
            "base_commit": base_commit,
            "defer_cancellation_terminal": true
        }),
    )?;
    settle_superseded_handoffs(&registry, target_root, goal_id, round_idx, &operation.id)?;
    Ok(operation)
}

pub(crate) fn find_candidate_handoff(
    runtime_root: &Path,
    target_root: &Path,
    goal_id: &str,
    round_idx: usize,
) -> RefineResult<Option<OperationHandle>> {
    active_handoff(
        &FileOperationRegistry::new(runtime_root),
        &handoff_owner(target_root, goal_id, round_idx),
    )
}

pub(crate) fn record_candidate_handoff_commit(
    runtime_root: &Path,
    operation_id: &str,
    candidate_commit: &str,
) -> RefineResult<OperationHandle> {
    FileOperationRegistry::new(runtime_root).update_progress(
        operation_id,
        json!({
            "stage": "candidate_committed",
            "candidate_commit": candidate_commit
        }),
    )
}

pub(crate) fn record_candidate_handoff_governance(
    runtime_root: &Path,
    operation_id: &str,
    candidate_commit: &str,
) -> RefineResult<OperationHandle> {
    FileOperationRegistry::new(runtime_root).update_progress(
        operation_id,
        json!({
            "stage": "governance_pending",
            "candidate_commit": candidate_commit
        }),
    )
}

pub(crate) fn settle_candidate_handoff(
    runtime_root: &Path,
    operation_id: &str,
    disposition: &str,
    evidence: Value,
) -> RefineResult<OperationHandle> {
    FileOperationRegistry::new(runtime_root).finish_with_result(
        operation_id,
        OperationState::Succeeded,
        json!({
            "disposition": disposition,
            "evidence": evidence
        }),
    )
}

pub(crate) fn retain_candidate_handoff_after_failure(
    runtime_root: &Path,
    operation_id: &str,
    code: &str,
    current_candidate_commit: Option<&str>,
    error: &RefineError,
) {
    let registry = FileOperationRegistry::new(runtime_root);
    let candidate_commit = current_candidate_commit
        .map(|commit| Value::String(commit.to_string()))
        .or_else(|| {
            registry
                .status(operation_id)
                .ok()
                .and_then(|operation| operation.progress.get("candidate_commit").cloned())
        });
    let _ = registry.update_progress(
        operation_id,
        json!({
            "stage": "workflow_failed_candidate_retained",
            "candidate_commit": candidate_commit,
            "failure": {
                "code": code,
                "message": error.to_string()
            }
        }),
    );
}

fn active_handoff(
    registry: &FileOperationRegistry,
    owner: &str,
) -> RefineResult<Option<OperationHandle>> {
    Ok(registry.recover()?.into_iter().find(|operation| {
        operation.owner == owner
            && matches!(
                operation.state,
                OperationState::Pending | OperationState::Running | OperationState::Cancelling
            )
            && operation.request.get("kind").and_then(Value::as_str) == Some(HANDOFF_KIND)
    }))
}

fn settle_superseded_handoffs(
    registry: &FileOperationRegistry,
    target_root: &Path,
    goal_id: &str,
    round_idx: usize,
    current_operation_id: &str,
) -> RefineResult<()> {
    for operation in registry.recover()?.into_iter().filter(|operation| {
        operation.id != current_operation_id
            && matches!(
                operation.state,
                OperationState::Pending | OperationState::Running | OperationState::Cancelling
            )
            && operation.request.get("kind").and_then(Value::as_str) == Some(HANDOFF_KIND)
            && operation.request.get("goal_id").and_then(Value::as_str) == Some(goal_id)
            && operation.request.get("target_root").and_then(Value::as_str) == target_root.to_str()
    }) {
        registry.finish_with_result(
            &operation.id,
            OperationState::Succeeded,
            json!({
                "disposition": "superseded_by_round",
                "evidence": {
                    "successor_round_idx": round_idx,
                    "successor_operation_id": current_operation_id
                }
            }),
        )?;
    }
    Ok(())
}

fn validate_handoff(
    operation: &OperationHandle,
    goal_id: &str,
    round_idx: usize,
    branch: &str,
    worktree_path: &str,
    base_commit: &str,
) -> RefineResult<()> {
    let exact = operation.request.get("goal_id").and_then(Value::as_str) == Some(goal_id)
        && operation.request.get("round_idx").and_then(Value::as_u64)
            == u64::try_from(round_idx).ok()
        && operation.request.get("branch").and_then(Value::as_str) == Some(branch)
        && operation
            .request
            .get("worktree_path")
            .and_then(Value::as_str)
            == Some(worktree_path)
        && operation.request.get("base_commit").and_then(Value::as_str) == Some(base_commit);
    if exact {
        Ok(())
    } else {
        Err(RefineError::Conflict(format!(
            "Candidate handoff {} does not match Goal {goal_id} Round {} branch {branch} path {worktree_path} base {base_commit}",
            operation.id,
            round_idx + 1
        )))
    }
}

fn handoff_owner(target_root: &Path, goal_id: &str, round_idx: usize) -> String {
    format!(
        "candidate_handoff:{}:{goal_id}:{round_idx}",
        target_root.display()
    )
}
