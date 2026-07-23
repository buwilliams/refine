use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::model::goal::RoundIntegration;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::workflow_subprocess_metadata;
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationState};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService, MergeResult};
use crate::tools::host::project_layout::target_root_for_refine_dir;
use crate::tools::product::project_state::GoalSummaryProjection;
use crate::tools::product::work_items::{FileWorkItemService, workflow_revision};
use crate::workflow::{WorkflowEngine, WorkflowExecutionFence};

#[derive(Clone, Debug)]
pub struct FileMergerService {
    pub runtime_root: PathBuf,
    pub refine_dir: PathBuf,
    pub target_root: Option<PathBuf>,
}

impl FileMergerService {
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

    /// Integrate the recorded automated workflow candidate during Ready Merge.
    ///
    /// The repository lock serializes fetch/merge/push across processes. Successful evidence is
    /// written before the caller advances the Goal, so a crash after push is recovered by proving
    /// that the exact candidate already belongs to the configured target branch.
    pub fn integrate_workflow_candidate(
        &self,
        goal_id: &str,
        round_idx: usize,
        claim_id: &str,
        execution_id: &str,
        node_id: &str,
        expected_branch: &str,
        expected_candidate: &str,
        expected_remote: &str,
    ) -> RefineResult<RoundIntegration> {
        let target_root = match &self.target_root {
            Some(target_root) => target_root.clone(),
            None => target_root(&self.refine_dir)?,
        };
        let fence = {
            let _coordination = acquire_workflow_coordination(&self.refine_dir)?;
            let detail = FileWorkItemService::for_node(&self.refine_dir, node_id)
                .show_goal_detail(goal_id)?;
            let goal_revision = workflow_revision(&detail);
            WorkflowEngine::with_target_root(&self.runtime_root, &target_root)
                .commit_ready_merge_fence(
                    claim_id,
                    execution_id,
                    goal_id,
                    node_id,
                    round_idx,
                    goal_revision,
                )?
        };
        let operations = FileOperationRegistry::new(&self.runtime_root);
        let operation = operations.register_exclusive_with_request(
            &format!("merger:{goal_id}:{}", round_idx + 1),
            json!({
                "goal_id": goal_id,
                "round_idx": round_idx,
                "claim_id": claim_id,
                "execution_id": execution_id,
                "node_id": node_id,
                "goal_revision": fence.goal_revision,
                "candidate_commit": expected_candidate,
                "branch": expected_branch,
                "remote": expected_remote
            }),
        )?;
        let result = with_repository_git_lock(&target_root, || {
            self.integrate_workflow_candidate_locked(
                &target_root,
                goal_id,
                round_idx,
                &fence,
                &operation.id,
                expected_branch,
                expected_candidate,
                expected_remote,
            )
        });
        match result {
            Ok(integration) => {
                operations.succeed_with_result_and_progress(
                    &operation.id,
                    json!({"stage": "settled"}),
                    json!({"integration": &integration}),
                )?;
                Ok(integration)
            }
            Err(error) => {
                let state = WorkflowEngine::with_target_root(&self.runtime_root, &target_root)
                    .load_state()
                    .ok()
                    .and_then(|state| {
                        state
                            .claims
                            .into_iter()
                            .find(|claim| claim.claim_id == claim_id)
                            .map(|claim| claim.state)
                    });
                if matches!(state, Some(crate::workflow::WorkflowClaimState::Cancelled)) {
                    let _ = operations.finish(&operation.id, OperationState::Cancelled);
                } else {
                    let _ = operations.fail_with_error(
                        &operation.id,
                        json!({
                            "code": "ready_merge_integration_failed",
                            "message": error.to_string(),
                            "execution_id": execution_id
                        }),
                    );
                }
                Err(error)
            }
        }
    }

    /// Accept a reviewed integration without performing Git integration again.
    pub fn approve_reviewed_goal(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
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
                "Goal {goal_id} reached review without successful Ready Merge evidence"
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
                git.fetch_branch(&integration.remote, &integration.target_branch)?;
                let published = git.resolve_commit(&format!(
                    "{}/{}",
                    integration.remote, integration.target_branch
                ))?;
                if !git.commit_is_ancestor(&candidate_commit, &published)? {
                    return Err(RefineError::Conflict(format!(
                        "Reviewed candidate {candidate_commit} is not published to {}/{}",
                        integration.remote, integration.target_branch
                    )));
                }
            }
            Ok(())
        })?;

        work_items.verify_goal_summary(goal_id)
    }

    fn integrate_workflow_candidate_locked(
        &self,
        target_root: &Path,
        goal_id: &str,
        round_idx: usize,
        fence: &WorkflowExecutionFence,
        operation_id: &str,
        expected_branch: &str,
        expected_candidate: &str,
        expected_remote: &str,
    ) -> RefineResult<RoundIntegration> {
        let work_items = FileWorkItemService::for_node(&self.refine_dir, &fence.node_id);
        self.verify_integration_fence(
            &work_items,
            fence,
            expected_branch,
            expected_candidate,
            expected_remote,
        )?;
        let goal = work_items.show_goal_summary(goal_id)?;
        let goal_node = goal.goal.node_id.as_deref().unwrap_or("default");
        if goal_node != fence.node_id {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is owned by node {goal_node}, not integration worker node {}",
                fence.node_id
            )));
        }
        let detail = work_items.show_goal_detail(goal_id)?;
        let rounds = detail
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no rounds")))?;
        if round_idx + 1 != rounds.len() {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} candidate round changed from {} to {} before integration",
                round_idx + 1,
                rounds.len()
            )));
        }
        let round = &rounds[round_idx];
        let branch_name = required_string(&detail, "branch_name", goal_id)?;
        let target_branch = required_string(&detail, "target_branch", goal_id)?;
        let base_commit = required_string(&detail, "base_commit", goal_id)?;
        let candidate_commit = required_string(&detail, "candidate_commit", goal_id)?;
        let remote = required_string(round, "workflow_git_remote", goal_id)?;
        for (label, recorded, expected) in [
            ("branch", branch_name.as_str(), expected_branch),
            ("candidate", candidate_commit.as_str(), expected_candidate),
            ("remote", remote.as_str(), expected_remote),
        ] {
            if recorded != expected {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} {label} changed before Ready Merge integration: recorded {recorded}, worker expected {expected}"
                )));
            }
        }
        let git = FileGitWorktreeService::with_runtime_root(target_root, &self.runtime_root)
            .with_operation_id(operation_id)
            .with_process_metadata(workflow_subprocess_metadata(
                &fence.execution_id,
                goal_id,
                "ready-merge",
                "WorkflowReadyMerge",
                Some(round_idx),
            ));
        if let Some(existing) = round_integration(round)? {
            self.verify_integration_fence(
                &work_items,
                fence,
                expected_branch,
                expected_candidate,
                expected_remote,
            )?;
            self.verify_existing_integration(&git, &existing)?;
            return Ok(existing);
        }
        if goal.goal.status != GoalStatus::ReadyMerge {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} cannot integrate from {}; expected ready-merge",
                goal.goal.status.as_str()
            )));
        }
        let remote_configured = git.remote_exists(&remote)?;
        if remote_configured {
            self.verify_integration_fence(
                &work_items,
                fence,
                expected_branch,
                expected_candidate,
                expected_remote,
            )?;
            git.fetch_branch(&remote, &branch_name)?;
            self.verify_integration_fence(
                &work_items,
                fence,
                expected_branch,
                expected_candidate,
                expected_remote,
            )?;
            git.ensure_branch_from_remote(&remote, &branch_name)?;
            let published = git.resolve_commit(&format!("{remote}/{branch_name}"))?;
            if published != candidate_commit {
                return Err(RefineError::Conflict(format!(
                    "Published candidate {branch_name} is {published}, expected {candidate_commit}"
                )));
            }
            self.verify_integration_fence(
                &work_items,
                fence,
                expected_branch,
                expected_candidate,
                expected_remote,
            )?;
            git.fetch_branch(&remote, &target_branch)?;
            let published_target = git.resolve_commit(&format!("{remote}/{target_branch}"))?;
            if git.commit_is_ancestor(&candidate_commit, &published_target)? {
                self.verify_integration_fence(
                    &work_items,
                    fence,
                    expected_branch,
                    expected_candidate,
                    expected_remote,
                )?;
                git.switch(&target_branch)?;
                self.verify_integration_fence(
                    &work_items,
                    fence,
                    expected_branch,
                    expected_candidate,
                    expected_remote,
                )?;
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
                            "Recovered successful Ready Merge integration from the published target branch"
                                .to_string(),
                        ),
                    },
                };
                self.verify_integration_fence(
                    &work_items,
                    fence,
                    expected_branch,
                    expected_candidate,
                    expected_remote,
                )?;
                self.persist_integration(&work_items, goal_id, round_idx, &recovered)?;
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
            return Err(RefineError::Conflict(format!(
                "Candidate {candidate_commit} is stale: recorded base {base_commit} is not its ancestor"
            )));
        }

        self.verify_integration_fence(
            &work_items,
            fence,
            expected_branch,
            expected_candidate,
            expected_remote,
        )?;
        git.switch(&target_branch)?;
        if remote_configured {
            let remote_target = git.resolve_commit(&format!("{remote}/{target_branch}"))?;
            let local_target = git.resolve_commit(&target_branch)?;
            if local_target != remote_target
                && !git.commit_is_ancestor(&remote_target, &local_target)?
            {
                self.verify_integration_fence(
                    &work_items,
                    fence,
                    expected_branch,
                    expected_candidate,
                    expected_remote,
                )?;
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
            self.verify_integration_fence(
                &work_items,
                fence,
                expected_branch,
                expected_candidate,
                expected_remote,
            )?;
            let merge = git.merge_commit_no_ff(&candidate_commit)?;
            if !merge.ok {
                let _ = git.recover();
                return Err(merge_failure("candidate integration", merge));
            }
            merge
        };
        let target_commit = git.resolve_commit(&target_branch)?;
        if remote_configured {
            self.verify_integration_fence(
                &work_items,
                fence,
                expected_branch,
                expected_candidate,
                expected_remote,
            )?;
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
        self.verify_integration_fence(
            &work_items,
            fence,
            expected_branch,
            expected_candidate,
            expected_remote,
        )?;
        self.persist_integration(&work_items, goal_id, round_idx, &integration)?;
        Ok(integration)
    }

    fn verify_integration_fence(
        &self,
        work_items: &FileWorkItemService,
        fence: &WorkflowExecutionFence,
        expected_branch: &str,
        expected_candidate: &str,
        expected_remote: &str,
    ) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.refine_dir)?;
        let target_root = match &self.target_root {
            Some(target_root) => target_root.clone(),
            None => target_root(&self.refine_dir)?,
        };
        WorkflowEngine::with_target_root(&self.runtime_root, target_root)
            .verify_ready_merge_fence(fence)?;
        let detail = work_items.show_goal_detail(&fence.goal_id)?;
        let actual_revision = workflow_revision(&detail);
        if actual_revision != fence.goal_revision {
            return Err(RefineError::Conflict(format!(
                "Goal {} changed from Ready Merge revision {} to {}",
                fence.goal_id, fence.goal_revision, actual_revision
            )));
        }
        if detail.get("status").and_then(Value::as_str) != Some(GoalStatus::ReadyMerge.as_str()) {
            return Err(RefineError::Conflict(format!(
                "Goal {} is no longer ready-merge",
                fence.goal_id
            )));
        }
        let rounds = detail
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Goal {} has no rounds", fence.goal_id))
            })?;
        if rounds.len() != fence.round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {} round changed before Ready Merge integration",
                fence.goal_id
            )));
        }
        for (label, recorded, expected) in [
            (
                "branch",
                detail.get("branch_name").and_then(Value::as_str),
                expected_branch,
            ),
            (
                "candidate",
                detail.get("candidate_commit").and_then(Value::as_str),
                expected_candidate,
            ),
            (
                "remote",
                rounds[fence.round_idx]
                    .get("workflow_git_remote")
                    .and_then(Value::as_str),
                expected_remote,
            ),
        ] {
            if recorded != Some(expected) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} {label} changed before Ready Merge integration",
                    fence.goal_id
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
                "Ready Merge evidence says candidate {} was integrated, but it is absent from {}",
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
            RefineError::Serialization(format!("invalid Ready Merge integration evidence: {error}"))
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
                "Goal {goal_id} has no recorded {key} for Ready Merge integration"
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
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::tools::host::project_layout::prepare_refine_dir;
    use crate::tools::product::work_items::FileWorkItemService;
    use crate::workflow::WorkflowAutomation;

    #[test]
    fn ready_merge_integrates_once_and_review_approval_only_accepts() {
        let temp_root = unique_temp_dir("ready-merge-integration");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("repo-refine-GOAL1-round-1");
        let remote = temp_root.join("remote.git");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        let refine_dir = prepare_refine_dir(&repo).unwrap();
        commit_file(&repo, "app.txt", "base\n", "initial");
        git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-u", "origin", "main"]).unwrap();

        let branch = "refine/GOAL1/round-1";
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        commit_file(&worktree_path, "feature.txt", "change\n", "GOAL1");
        git(&worktree_path, &["push", "-u", "origin", branch]).unwrap();
        let base_commit = git_stdout(&repo, &["rev-parse", "main"]);
        let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("GOAL1", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
            .unwrap();
        work_items
            .update_goal_git_refs(
                "GOAL1",
                branch,
                "main",
                &base_commit,
                Some(&candidate_commit),
            )
            .unwrap();
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({"workflow_git_remote": "origin"}),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
            .unwrap();

        let (claim_id, execution_id) = start_ready_merge_claim(&runtime_root, &repo);
        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let wrong_round = merger
            .integrate_workflow_candidate(
                "GOAL1",
                1,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap_err();
        assert!(
            wrong_round.to_string().contains("round changed"),
            "{wrong_round}"
        );
        assert!(
            merger
                .integrate_workflow_candidate(
                    "GOAL1",
                    0,
                    &claim_id,
                    &execution_id,
                    "other-node",
                    branch,
                    &candidate_commit,
                    "origin",
                )
                .unwrap_err()
                .to_string()
                .contains("no longer owns active claim")
        );
        let first = merger.clone();
        let second = merger.clone();
        let (first_result, second_result) = thread::scope(|scope| {
            let first = scope.spawn(|| {
                first.integrate_workflow_candidate(
                    "GOAL1",
                    0,
                    &claim_id,
                    &execution_id,
                    "default",
                    branch,
                    &candidate_commit,
                    "origin",
                )
            });
            let second = scope.spawn(|| {
                second.integrate_workflow_candidate(
                    "GOAL1",
                    0,
                    &claim_id,
                    &execution_id,
                    "default",
                    branch,
                    &candidate_commit,
                    "origin",
                )
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        let (integrated, concurrent_error) = match (first_result, second_result) {
            (Ok(integrated), Err(error)) | (Err(error), Ok(integrated)) => (integrated, error),
            unexpected => panic!("expected one serialized integration owner: {unexpected:?}"),
        };
        assert!(integrated.merge.ok);
        assert!(
            concurrent_error
                .to_string()
                .contains("already owns merger:GOAL1:1")
        );
        let repeated = merger
            .integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap();
        assert_eq!(repeated, integrated);
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({"workflow_integration": null}),
            )
            .unwrap();
        let recovered = merger
            .integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap();
        assert_eq!(recovered.candidate_commit, candidate_commit);
        assert_eq!(recovered.target_commit, integrated.target_commit);
        assert!(recovered.pushed);
        assert!(worktree_path.exists());
        assert_eq!(
            fs::read_to_string(repo.join("feature.txt")).unwrap(),
            "change\n"
        );
        let reviewed_head = git_stdout(&repo, &["rev-parse", "HEAD"]);
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Build)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Qa)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Review)
            .unwrap();

        let approved = merger.approve_reviewed_goal("GOAL1").unwrap();
        assert_eq!(approved.goal.status, GoalStatus::Done);
        assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), reviewed_head);
        assert!(worktree_path.exists());
        assert!(git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains(branch));
        assert!(git_succeeds(
            &repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
        ));
        assert_eq!(
            fs::read_to_string(repo.join("feature.txt")).unwrap(),
            "change\n"
        );
        let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
        assert!(git_stdout(&repo, &["ls-remote", "origin", "refs/heads/main"]).starts_with(&head));
        let audit = fs::read_to_string(repo.join(".git/refine-audit.jsonl")).unwrap();
        assert_eq!(
            audit
                .lines()
                .filter(|line| line.contains("\"action\":\"push\""))
                .count(),
            1,
            "Ready Merge must be the only target-branch push"
        );
        work_items.undo_goal_summary("GOAL1").unwrap();
        let next_round = work_items
            .append_goal_round_summary("GOAL1", "Reviewer", "Address review feedback")
            .unwrap();
        assert_eq!(next_round.goal.status, GoalStatus::Todo);
        assert_eq!(next_round.goal.round_count, 2);
        let next_detail = work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(
            next_detail["rounds"][0]["workflow_integration"]["target_commit"],
            integrated.target_commit
        );
        assert!(next_detail["rounds"][1]["workflow_integration"].is_null());
        assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), reviewed_head);
        let audit = fs::read_to_string(repo.join(".git/refine-audit.jsonl")).unwrap();
        assert_eq!(
            audit
                .lines()
                .filter(|line| line.contains("\"action\":\"push\""))
                .count(),
            1,
            "opening another round must not repeat target integration"
        );

        git(
            &repo,
            &[
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn ready_merge_conflict_aborts_without_advancing_or_losing_candidate() {
        let temp_root = unique_temp_dir("ready-merge-conflict");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("candidate");
        let remote = temp_root.join("remote.git");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        let refine_dir = prepare_refine_dir(&repo).unwrap();
        commit_file(&repo, "app.txt", "base\n", "initial");
        let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-u", "origin", "main"]).unwrap();
        let branch = "refine/GOAL1/round-1";
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        commit_file(&worktree_path, "app.txt", "candidate\n", "candidate");
        let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);
        git(&worktree_path, &["push", "-u", "origin", branch]).unwrap();
        commit_file(&repo, "app.txt", "target\n", "target");
        let target_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["push", "origin", "main"]).unwrap();

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("GOAL1", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
            .unwrap();
        work_items
            .update_goal_git_refs(
                "GOAL1",
                branch,
                "main",
                &base_commit,
                Some(&candidate_commit),
            )
            .unwrap();
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({"workflow_git_remote": "origin"}),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
            .unwrap();

        let (claim_id, execution_id) = start_ready_merge_claim(&runtime_root, &repo);
        let error = FileMergerService::new(&runtime_root, &refine_dir)
            .integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("candidate integration failed"),
            "{error}"
        );
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::ReadyMerge
        );
        assert!(
            work_items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_integration"]
                .is_null()
        );
        assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), target_commit);
        assert_eq!(
            fs::read_to_string(repo.join("app.txt")).unwrap(),
            "target\n"
        );
        assert_eq!(
            fs::read_to_string(worktree_path.join("app.txt")).unwrap(),
            "candidate\n"
        );
        assert!(git_stdout(&repo, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
        assert!(!git_succeeds(
            &repo,
            &["rev-parse", "--verify", "MERGE_HEAD"]
        ));
        assert!(!git_succeeds(
            &repo,
            &[
                "merge-base",
                "--is-ancestor",
                &candidate_commit,
                "origin/main"
            ]
        ));

        git(
            &repo,
            &[
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn ready_merge_push_failure_retries_without_duplicate_merge() {
        let temp_root = unique_temp_dir("ready-merge-push-retry");
        let repo = temp_root.join("repo");
        let refine_dir = repo.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let worktree_path = temp_root.join("candidate");
        let remote = temp_root.join("remote.git");
        fs::create_dir_all(&refine_dir).unwrap();
        init_repo(&repo);
        let refine_dir = prepare_refine_dir(&repo).unwrap();
        commit_file(&repo, "app.txt", "base\n", "initial");
        let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-u", "origin", "main"]).unwrap();
        let branch = "refine/GOAL1/round-1";
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        commit_file(&worktree_path, "feature.txt", "candidate\n", "candidate");
        let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);
        git(&worktree_path, &["push", "-u", "origin", branch]).unwrap();
        let hook = remote.join("hooks/pre-receive");
        fs::write(
            &hook,
            "#!/bin/sh\nwhile read old new ref; do\n  test \"$ref\" != refs/heads/main || exit 1\ndone\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&hook).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&hook, permissions).unwrap();
        }

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("GOAL1", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
            .unwrap();
        work_items
            .update_goal_git_refs(
                "GOAL1",
                branch,
                "main",
                &base_commit,
                Some(&candidate_commit),
            )
            .unwrap();
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({"workflow_git_remote": "origin"}),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
            .unwrap();
        let (claim_id, execution_id) = start_ready_merge_claim(&runtime_root, &repo);
        let merger = FileMergerService::new(&runtime_root, &refine_dir);
        let error = merger
            .integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("pre-receive hook declined"),
            "{error}"
        );
        let integrated_head = git_stdout(&repo, &["rev-parse", "HEAD"]);
        assert!(git_succeeds(
            &repo,
            &[
                "merge-base",
                "--is-ancestor",
                &candidate_commit,
                &integrated_head
            ]
        ));
        assert!(!git_succeeds(
            &repo,
            &[
                "merge-base",
                "--is-ancestor",
                &candidate_commit,
                "origin/main"
            ]
        ));
        assert!(
            work_items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_integration"]
                .is_null()
        );

        fs::remove_file(&hook).unwrap();
        let retried = merger
            .integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap();
        assert_eq!(retried.target_commit, integrated_head);
        assert!(retried.pushed);
        assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), integrated_head);
        assert!(git_succeeds(
            &repo,
            &[
                "merge-base",
                "--is-ancestor",
                &candidate_commit,
                "origin/main"
            ]
        ));
        let audit = fs::read_to_string(repo.join(".git/refine-audit.jsonl")).unwrap();
        assert_eq!(
            audit
                .lines()
                .filter(|line| line.contains("\"action\":\"merge_commit_no_ff\""))
                .count(),
            1
        );

        git(
            &repo,
            &[
                "worktree",
                "remove",
                "--force",
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::remove_dir_all(temp_root).unwrap();
    }

    fn init_repo(repo: &Path) {
        git(repo, &["init", "-b", "main"]).unwrap();
        git(repo, &["config", "user.email", "test@example.com"]).unwrap();
        git(repo, &["config", "user.name", "Test User"]).unwrap();
    }

    fn start_ready_merge_claim(runtime_root: &Path, repo: &Path) -> (String, String) {
        let automation = WorkflowEngine::with_target_root(runtime_root, repo);
        let claim_id = automation.claim("GOAL1").unwrap();
        let execution_id = automation.start_claim(&claim_id).unwrap();
        (claim_id, execution_id)
    }

    fn commit_file(repo: &Path, path: &str, contents: &str, message: &str) {
        fs::write(repo.join(path), contents).unwrap();
        git(repo, &["add", path]).unwrap();
        git(repo, &["commit", "-m", message]).unwrap();
    }

    fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Conflict(
                format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )
                .trim()
                .to_string(),
            ))
        }
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn git_succeeds(repo: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
