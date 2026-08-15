use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::model::goal::RoundIntegration;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::workflow_subprocess_metadata;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService, MergeResult};
use crate::tools::host::project_layout::target_root_for_refine_dir;
use crate::tools::product::project_projection::GoalSummaryProjection;
use crate::tools::product::work_items::FileWorkItemService;

mod reconciliation;

pub use reconciliation::{AlreadyMergedResolution, AlreadyMergedResolutionDisposition};

#[derive(Clone, Debug)]
pub struct FileGovernanceIntegrationService {
    pub runtime_root: PathBuf,
    pub refine_dir: PathBuf,
    pub target_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationRevert {
    pub merge_commit: String,
    pub revert_commit: String,
    pub result: MergeResult,
}

pub struct ReconciliationRequest<'a> {
    pub goal_id: &'a str,
    pub round_idx: usize,
    pub node_id: &'a str,
    pub integration: &'a RoundIntegration,
    pub expected_target_commit: &'a str,
}

#[derive(Clone, Debug)]
struct GovernanceAuthority {
    goal_id: String,
    node_id: String,
    round_idx: usize,
    branch: String,
    candidate: String,
    remote: String,
}

impl FileGovernanceIntegrationService {
    pub fn new(runtime_root: impl Into<PathBuf>, refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: refine_dir.into(),
            target_root: None,
        }
    }

    pub fn with_target_root(
        runtime_root: impl Into<PathBuf>,
        refine_dir: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: refine_dir.into(),
            target_root: Some(target_root.into()),
        }
    }

    /// Integrate the recorded automated workflow candidate during Governance.
    ///
    /// The repository lock serializes fetch/merge/push across processes. Successful evidence is
    /// written before the caller advances the Goal, so a crash after push is recovered by proving
    /// that the exact candidate already belongs to the configured target branch.
    pub fn integrate_workflow_candidate(
        &self,
        goal_id: &str,
        round_idx: usize,
        node_id: &str,
        expected_branch: &str,
        expected_candidate: &str,
        expected_remote: &str,
    ) -> RefineResult<RoundIntegration> {
        self.integrate_workflow_candidate_and_settle(
            goal_id,
            round_idx,
            node_id,
            expected_branch,
            expected_candidate,
            expected_remote,
            |_| Ok(()),
        )
        .map(|(integration, ())| integration)
    }

    /// Integrates a Governance candidate and orders its operation settlement with the caller's
    /// next workflow transition.
    ///
    /// Goal authority is checked once more under the repository lock immediately before the first
    /// Git effect. From that point onward the integration is allowed to finish even if the Goal is
    /// concurrently cancelled, because rolling back a partially published Git operation is less
    /// reliable than recording its exact result.
    // Keep every synchronized authority input explicit at the side-effect boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn integrate_workflow_candidate_and_settle<T>(
        &self,
        goal_id: &str,
        round_idx: usize,
        node_id: &str,
        expected_branch: &str,
        expected_candidate: &str,
        expected_remote: &str,
        settlement: impl FnOnce(&RoundIntegration) -> RefineResult<T>,
    ) -> RefineResult<(RoundIntegration, T)> {
        let target_root = match &self.target_root {
            Some(target_root) => target_root.clone(),
            None => target_root(&self.refine_dir)?,
        };
        let authority = GovernanceAuthority {
            goal_id: goal_id.to_string(),
            node_id: node_id.to_string(),
            round_idx,
            branch: expected_branch.to_string(),
            candidate: expected_candidate.to_string(),
            remote: expected_remote.to_string(),
        };
        let work_items = FileWorkItemService::for_node(&self.refine_dir, node_id);
        self.verify_integration_authority(&work_items, &authority, true)?;
        let operations = FileOperationRegistry::new(&self.runtime_root);
        let mut operation_id = None;
        let mut settlement = Some(settlement);
        let result = with_repository_git_lock(&target_root, || {
            let detail = work_items.show_goal_detail(goal_id)?;
            if let Some(existing) = detail
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|rounds| rounds.get(round_idx))
                .map(round_integration)
                .transpose()?
                .flatten()
            {
                self.verify_integration_authority(&work_items, &authority, false)?;
                let git =
                    FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
                self.verify_existing_integration(&git, &existing)?;
                let transitioned = settlement
                    .take()
                    .expect("Governance settlement is called once")(
                    &existing
                )?;
                return Ok((existing, transitioned));
            }
            let operation = operations.register_exclusive_with_request(
                &format!("governance-integration:{goal_id}:{}", round_idx + 1),
                json!({
                    "goal_id": goal_id,
                    "round_idx": round_idx,
                    "node_id": node_id,
                    "candidate_commit": expected_candidate,
                    "branch": expected_branch,
                    "remote": expected_remote
                }),
            )?;
            operation_id = Some(operation.id.clone());
            let integration =
                self.integrate_workflow_candidate_locked(&target_root, &authority, &operation.id)?;
            let (_, transitioned) = operations.succeed_after(
                &operation.id,
                json!({"stage": "settled"}),
                json!({"integration": &integration}),
                || {
                    settlement
                        .take()
                        .expect("Governance settlement is called once")(
                        &integration
                    )
                },
            )?;
            Ok((integration, transitioned))
        });
        match result {
            Ok(settled) => Ok(settled),
            Err(error) => {
                let operation_state = operation_id
                    .as_deref()
                    .and_then(|operation_id| operations.status(operation_id).ok());
                let cancelled = operation_state
                    .as_ref()
                    .is_some_and(|operation| operation.state == OperationState::Cancelled)
                    || work_items
                        .show_goal_summary(goal_id)
                        .is_ok_and(|goal| goal.goal.status == GoalStatus::Cancelled);
                if cancelled {
                    // Cancellation is already durable and authoritative. Preserve its causal
                    // operation error instead of replacing it with a late Git/settlement failure.
                    return Err(RefineError::Conflict(format!(
                        "Governance for Goal {goal_id} was cancelled: {error}"
                    )));
                } else {
                    let code = if matches!(&error, RefineError::StaleCandidate { .. }) {
                        "governance_candidate_stale"
                    } else {
                        "governance_integration_failed"
                    };
                    if let Some(operation_id) = operation_id.as_deref() {
                        let _ = operations.fail_with_error(
                            operation_id,
                            json!({
                                "code": code,
                                "message": error.to_string()
                            }),
                        );
                    }
                }
                Err(error)
            }
        }
    }

    /// Accept a reviewed integration without performing Git integration again.
    pub fn approve_reviewed_goal(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let goal = work_items.show_goal_summary(goal_id)?;
        if goal.goal.status != GoalStatus::Review {
            return Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} can only be approved from review"
            )));
        }
        let detail = work_items.show_goal_detail(goal_id)?;
        let candidate_commit = detail
            .get("candidate_commit")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no exact candidate commit to accept"
                ))
            })?;
        let round = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.last())
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no review round")))?;
        let integration = round_integration(round)?.ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} reached review without successful Governance evidence"
            ))
        })?;
        if integration.candidate_commit != candidate_commit {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} review candidate changed from integrated commit {} to {}",
                integration.candidate_commit, candidate_commit
            )));
        }
        let target_root = match &self.target_root {
            Some(target_root) => target_root.clone(),
            None => target_root(&self.refine_dir)?,
        };
        with_repository_git_lock(&target_root, || {
            let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
            let target_commit = git.resolve_commit(&integration.target_branch)?;
            if !git.commit_is_ancestor(&candidate_commit, &target_commit)? {
                return Err(RefineError::Conflict(format!(
                    "Reviewed candidate {candidate_commit} is not integrated in {}",
                    integration.target_branch
                )));
            }
            if integration.pushed {
                // Refine pushed the candidate itself, so the local tracking ref
                // normally already proves publication; a network fetch inside
                // this request — while holding the repository lock — is only
                // worth its latency when that local evidence is insufficient.
                let remote_ref =
                    format!("{}/{}", integration.remote, integration.target_branch);
                let published_locally = match git.resolve_commit(&remote_ref) {
                    Ok(published) => git.commit_is_ancestor(&candidate_commit, &published)?,
                    Err(_) => false,
                };
                if !published_locally {
                    git.fetch_branch(&integration.remote, &integration.target_branch)?;
                    let published = git.resolve_commit(&remote_ref)?;
                    if !git.commit_is_ancestor(&candidate_commit, &published)? {
                        return Err(RefineError::Conflict(format!(
                            "Reviewed candidate {candidate_commit} is not published to {}/{}",
                            integration.remote, integration.target_branch
                        )));
                    }
                }
            }
            Ok(())
        })?;

        work_items.verify_goal_summary(goal_id)
    }

    /// Revert one already-integrated Goal without rewriting shared history.
    ///
    /// Automatic reconciliation is intentionally limited to integration evidence that identifies
    /// an exact two-parent merge commit whose first parent did not contain the candidate and whose
    /// second parent does. That proves which shared-branch delta belongs to this Goal. Any
    /// ambiguity, dirty target, concurrent branch movement, or revert conflict is surfaced without
    /// pushing or changing Goal status.
    pub fn revert_reconciled_candidate_and_settle<T>(
        &self,
        request: ReconciliationRequest<'_>,
        settlement: impl FnOnce(&ReconciliationRevert) -> RefineResult<T>,
    ) -> RefineResult<(ReconciliationRevert, T)> {
        let ReconciliationRequest {
            goal_id,
            round_idx,
            node_id,
            integration,
            expected_target_commit,
        } = request;
        let target_root = match &self.target_root {
            Some(target_root) => target_root.clone(),
            None => target_root(&self.refine_dir)?,
        };
        let work_items = FileWorkItemService::for_node(&self.refine_dir, node_id);
        let summary = work_items.show_goal_summary(goal_id)?;
        if summary.goal.status != GoalStatus::Quality {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from quality to {} before its reconciliation revert",
                summary.goal.status.as_str()
            )));
        }
        if summary.goal.node_id.as_deref().unwrap_or("default") != node_id
            || summary.goal.round_count != round_idx + 1
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} no longer authorizes reconciliation on node {node_id} round {}",
                round_idx + 1
            )));
        }
        let detail = work_items.show_goal_detail(goal_id)?;
        let round = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(round_idx))
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no round {} for reconciliation",
                    round_idx + 1
                ))
            })?;
        let recorded_integration = round_integration(round)?.ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no Governance evidence for reconciliation"
            ))
        })?;
        if &recorded_integration != integration {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} integration evidence changed before its reconciliation revert"
            )));
        }
        with_repository_git_lock(&target_root, || {
            let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
            let status = git.inspect(target_root.to_str().unwrap_or(""))?;
            if status.branch.as_deref() != Some(integration.target_branch.as_str()) {
                return Err(RefineError::Conflict(format!(
                    "already-merged reconciliation requires target worktree {} to be on branch {}, found {}; user work was preserved",
                    target_root.display(),
                    integration.target_branch,
                    status.branch.as_deref().unwrap_or("<detached>")
                )));
            }
            if status.dirty_user_changes || !status.refine_owned_artifacts.is_empty() {
                return Err(RefineError::Conflict(format!(
                    "already-merged reconciliation found a dirty target index or worktree at {}; user work was preserved",
                    target_root.display()
                )));
            }
            let current_target = git.resolve_commit(&integration.target_branch)?;
            if current_target != expected_target_commit {
                return Err(RefineError::Conflict(format!(
                    "already-merged reconciliation target changed from {expected_target_commit} to {current_target}; no revert was attempted"
                )));
            }
            if integration.pushed {
                git.fetch_branch(&integration.remote, &integration.target_branch)?;
                let published = git.resolve_commit(&format!(
                    "{}/{}",
                    integration.remote, integration.target_branch
                ))?;
                if published != expected_target_commit {
                    return Err(RefineError::Conflict(format!(
                        "published target {}/{} changed from {expected_target_commit} to {published}; no revert was attempted",
                        integration.remote, integration.target_branch
                    )));
                }
            }
            if !git.commit_is_ancestor(&integration.target_commit, &current_target)? {
                return Err(RefineError::Conflict(format!(
                    "recorded integration commit {} is absent from {}; no revert was attempted",
                    integration.target_commit, integration.target_branch
                )));
            }
            let parents = git.commit_parents(&integration.target_commit)?;
            if parents.len() != 2
                || git.commit_is_ancestor(&integration.candidate_commit, &parents[0])?
                || !git.commit_is_ancestor(&integration.candidate_commit, &parents[1])?
            {
                return Err(RefineError::Conflict(format!(
                    "recorded integration commit {} does not uniquely identify a two-parent merge for candidate {}; no revert was attempted",
                    integration.target_commit, integration.candidate_commit
                )));
            }
            let result = git.revert_merge_commit(&integration.target_commit, 1)?;
            if !result.ok {
                let _ = git.recover();
                return Err(merge_failure(
                    "already-merged reconciliation revert",
                    result,
                ));
            }
            let revert_commit = git.resolve_commit(&integration.target_branch)?;
            if integration.pushed
                && let Err(error) = git.push(&integration.remote, &integration.target_branch)
            {
                let rollback = git.reset_hard_to(expected_target_commit);
                if let Err(rollback) = rollback {
                    return Err(RefineError::Conflict(format!(
                        "reconciliation revert {revert_commit} could not be pushed to {}/{} ({error}) and the clean local target could not be restored to {expected_target_commit} ({rollback})",
                        integration.remote, integration.target_branch
                    )));
                }
                return Err(RefineError::Conflict(format!(
                    "reconciliation revert {revert_commit} could not be pushed to {}/{}; the clean local target was restored to {expected_target_commit}: {error}",
                    integration.remote, integration.target_branch
                )));
            }
            let reverted = ReconciliationRevert {
                merge_commit: integration.target_commit.clone(),
                revert_commit,
                result,
            };
            let settled = settlement(&reverted)?;
            Ok((reverted, settled))
        })
    }

    fn integrate_workflow_candidate_locked(
        &self,
        target_root: &Path,
        authority: &GovernanceAuthority,
        operation_id: &str,
    ) -> RefineResult<RoundIntegration> {
        let work_items = FileWorkItemService::for_node(&self.refine_dir, &authority.node_id);
        // This is the last authority check before the first Git side effect. Cancellation or
        // reassignment before this point prevents integration. Once this succeeds, the repository
        // operation is allowed to finish and record evidence without a rollback protocol.
        self.verify_integration_authority(&work_items, authority, true)?;
        let detail = work_items.show_goal_detail(&authority.goal_id)?;
        let rounds = detail
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Goal {} has no rounds", authority.goal_id))
            })?;
        let round = &rounds[authority.round_idx];
        let branch_name = required_string(&detail, "branch_name", &authority.goal_id)?;
        let target_branch = required_string(&detail, "target_branch", &authority.goal_id)?;
        let base_commit = required_string(&detail, "base_commit", &authority.goal_id)?;
        let candidate_commit = required_string(&detail, "candidate_commit", &authority.goal_id)?;
        let remote = required_string(round, "workflow_git_remote", &authority.goal_id)?;
        let mut process_metadata = workflow_subprocess_metadata(
            &authority.goal_id,
            "governance",
            "WorkflowGovernance",
            Some(authority.round_idx),
        );
        // Goal cancellation must not kill a Git child after the Governance point of no return.
        // This is node-local process metadata, not synchronized execution authority.
        process_metadata.insert("side_effect_committed".to_string(), json!(true));
        let git = FileGitWorktreeService::with_runtime_root(target_root, &self.runtime_root)
            .with_operation_id(operation_id)
            .with_process_metadata(process_metadata);
        if let Some(existing) = round_integration(round)? {
            self.verify_existing_integration(&git, &existing)?;
            return Ok(existing);
        }
        let remote_configured = git.remote_exists(&remote)?;
        if remote_configured {
            git.fetch_branch(&remote, &branch_name)?;
            git.ensure_branch_from_remote(&remote, &branch_name)?;
            let published = git.resolve_commit(&format!("{remote}/{branch_name}"))?;
            if published != candidate_commit {
                return Err(RefineError::Conflict(format!(
                    "Published candidate {branch_name} is {published}, expected {candidate_commit}"
                )));
            }
            git.fetch_branch(&remote, &target_branch)?;
            let published_target = git.resolve_commit(&format!("{remote}/{target_branch}"))?;
            if git.commit_is_ancestor(&candidate_commit, &published_target)? {
                git.switch(&target_branch)?;
                git.fast_forward_from_remote(&remote, &target_branch)?;
                let recovered = RoundIntegration {
                    candidate_commit,
                    target_branch,
                    target_commit: published_target,
                    remote,
                    pushed: true,
                    integrated_at: Utc::now().to_rfc3339(),
                    merge: MergeResult {
                        ok: true,
                        conflicts: Vec::new(),
                        message: Some(
                            "Recovered successful Governance integration from the published target branch"
                                .to_string(),
                        ),
                    },
                };
                self.persist_integration(
                    &work_items,
                    &authority.goal_id,
                    authority.round_idx,
                    &recovered,
                )?;
                return Ok(recovered);
            }
        }

        let resolved_candidate = git.resolve_commit(&candidate_commit)?;
        if resolved_candidate != candidate_commit {
            return Err(RefineError::Conflict(format!(
                "Candidate commit {candidate_commit} resolved unexpectedly to {resolved_candidate}"
            )));
        }
        if !git.commit_is_ancestor(&base_commit, &candidate_commit)? {
            // A candidate already contained in the target branch is integrated,
            // not stale: the work is in the branch whatever the recorded base
            // says, and there is nothing left to merge. Integration below
            // recognizes that and still publishes it, but only if the candidate
            // gets that far — rejecting here first turned finished work into a
            // failed Goal whenever the branch tip moved under an in-flight round.
            // Reject only when the work is genuinely absent from the target.
            let local_target = git.resolve_commit(&target_branch)?;
            if !git.commit_is_ancestor(&candidate_commit, &local_target)? {
                return Err(RefineError::StaleCandidate {
                    candidate_commit,
                    recorded_base: base_commit,
                    target_branch,
                    target_commit: local_target,
                });
            }
        }

        git.switch(&target_branch)?;
        if remote_configured {
            let remote_target = git.resolve_commit(&format!("{remote}/{target_branch}"))?;
            let local_target = git.resolve_commit(&target_branch)?;
            if local_target != remote_target
                && !git.commit_is_ancestor(&remote_target, &local_target)?
            {
                let synchronized = git.merge_commit_no_ff(&remote_target)?;
                if !synchronized.ok {
                    let _ = git.recover();
                    return Err(merge_failure("target synchronization", synchronized));
                }
            }
        }

        let current_target = git.resolve_commit(&target_branch)?;
        let merge = if git.commit_is_ancestor(&candidate_commit, &current_target)? {
            MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some(
                    "Exact candidate was already present in the local target branch".to_string(),
                ),
            }
        } else {
            let merge = git.merge_commit_no_ff(&candidate_commit)?;
            if !merge.ok {
                let _ = git.recover();
                return Err(merge_failure("candidate integration", merge));
            }
            merge
        };
        let target_commit = git.resolve_commit(&target_branch)?;
        if remote_configured {
            git.push(&remote, &target_branch)?;
        }
        let integration = RoundIntegration {
            candidate_commit,
            target_branch,
            target_commit,
            remote,
            pushed: remote_configured,
            integrated_at: Utc::now().to_rfc3339(),
            merge,
        };
        self.persist_integration(
            &work_items,
            &authority.goal_id,
            authority.round_idx,
            &integration,
        )?;
        Ok(integration)
    }

    fn verify_integration_authority(
        &self,
        work_items: &FileWorkItemService,
        authority: &GovernanceAuthority,
        require_governance: bool,
    ) -> RefineResult<()> {
        let detail = work_items.show_goal_detail(&authority.goal_id)?;
        let status = detail.get("status").and_then(Value::as_str);
        if require_governance && status != Some(GoalStatus::Governance.as_str()) {
            return Err(RefineError::Conflict(format!(
                "Goal {} is no longer in governance",
                authority.goal_id
            )));
        }
        if !require_governance
            && !matches!(
                status,
                Some("governance" | "review" | "cancelled" | "ready-merge" | "build")
            )
        {
            return Err(RefineError::Conflict(format!(
                "Goal {} no longer authorizes Governance recovery from status {}",
                authority.goal_id,
                status.unwrap_or("unknown")
            )));
        }
        let node = detail
            .get("node_id")
            .and_then(Value::as_str)
            .unwrap_or("default");
        if node != authority.node_id {
            return Err(RefineError::Conflict(format!(
                "Goal {} is assigned to node {node}, not {}",
                authority.goal_id, authority.node_id
            )));
        }
        let rounds = detail
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Goal {} has no rounds", authority.goal_id))
            })?;
        if rounds.len() != authority.round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {} round changed before Governance integration",
                authority.goal_id
            )));
        }
        for (label, recorded, expected) in [
            (
                "branch",
                detail.get("branch_name").and_then(Value::as_str),
                authority.branch.as_str(),
            ),
            (
                "candidate",
                detail.get("candidate_commit").and_then(Value::as_str),
                authority.candidate.as_str(),
            ),
            (
                "remote",
                rounds[authority.round_idx]
                    .get("workflow_git_remote")
                    .and_then(Value::as_str),
                authority.remote.as_str(),
            ),
        ] {
            if recorded != Some(expected) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} {label} changed before Governance integration",
                    authority.goal_id
                )));
            }
        }
        Ok(())
    }

    fn persist_integration(
        &self,
        work_items: &FileWorkItemService,
        goal_id: &str,
        round_idx: usize,
        integration: &RoundIntegration,
    ) -> RefineResult<()> {
        work_items.update_goal_round_evaluation_summary(
            goal_id,
            round_idx,
            &json!({"workflow_integration": integration}),
        )?;
        Ok(())
    }

    fn verify_existing_integration(
        &self,
        git: &FileGitWorktreeService,
        integration: &RoundIntegration,
    ) -> RefineResult<()> {
        let target = if integration.pushed {
            git.fetch_branch(&integration.remote, &integration.target_branch)?;
            git.resolve_commit(&format!(
                "{}/{}",
                integration.remote, integration.target_branch
            ))?
        } else {
            git.resolve_commit(&integration.target_branch)?
        };
        if !git.commit_is_ancestor(&integration.candidate_commit, &target)? {
            return Err(RefineError::Conflict(format!(
                "Governance evidence says candidate {} was integrated, but it is absent from {}",
                integration.candidate_commit, integration.target_branch
            )));
        }
        Ok(())
    }
}

fn round_integration(round: &Value) -> RefineResult<Option<RoundIntegration>> {
    let Some(value) = round
        .get("workflow_integration")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|error| {
            RefineError::Serialization(format!("invalid Governance integration evidence: {error}"))
        })
}

fn required_string(value: &Value, key: &str, goal_id: &str) -> RefineResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no recorded {key} for Governance integration"
            ))
        })
}

fn merge_failure(stage: &str, merge: MergeResult) -> RefineError {
    RefineError::Conflict(format!(
        "{stage} failed: {}",
        merge
            .message
            .unwrap_or_else(|| "Git merge failed".to_string())
    ))
}

pub fn branch_name_for_goal(settings: &JsonObject, goal_id: &str) -> String {
    setting_string(settings, "branch_name_pattern", "refine/{goal_id}")
        .replace("{goal_id}", goal_id)
}

pub fn target_root(refine_dir: &Path) -> RefineResult<PathBuf> {
    target_root_for_refine_dir(refine_dir)
}

fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests;
