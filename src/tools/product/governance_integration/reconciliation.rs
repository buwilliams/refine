use serde_json::{Value, json};

use super::*;
use crate::tools::product::work_items::{
    AlreadyMergedInspection, AlreadyMergedResolutionSnapshot, AlreadyMergedSettlement,
    AlreadyMergedSettlementDecision, WorkflowAttemptAuthority,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlreadyMergedResolutionDisposition {
    Resolved,
    AlreadyResolved,
    Failed,
    AlreadyFailed,
}

impl AlreadyMergedResolutionDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::AlreadyResolved => "already_resolved",
            Self::Failed => "failed",
            Self::AlreadyFailed => "already_failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AlreadyMergedResolution {
    pub disposition: AlreadyMergedResolutionDisposition,
    pub goal: GoalSummaryProjection,
    pub evidence: Value,
}

#[derive(Clone, Debug, Default)]
struct GitObservation {
    local_target_commit: Option<String>,
    published_target_commit: Option<String>,
    error: Option<String>,
}

impl FileGovernanceIntegrationService {
    pub fn resolve_already_merged_goal(
        &self,
        goal_id: &str,
    ) -> RefineResult<AlreadyMergedResolution> {
        self.resolve_already_merged_goal_inner(goal_id, None)
    }

    pub(crate) fn resolve_already_merged_goal_with_authority(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
    ) -> RefineResult<AlreadyMergedResolution> {
        self.resolve_already_merged_goal_inner(goal_id, Some(authority))
    }

    fn resolve_already_merged_goal_inner(
        &self,
        goal_id: &str,
        expected_authority: Option<WorkflowAttemptAuthority>,
    ) -> RefineResult<AlreadyMergedResolution> {
        let target_root = match &self.target_root {
            Some(target_root) => target_root.clone(),
            None => target_root(&self.refine_dir)?,
        };
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let snapshot = match work_items.inspect_already_merged_resolution(goal_id)? {
            AlreadyMergedInspection::AlreadyResolved(goal, evidence) => {
                return Ok(AlreadyMergedResolution {
                    disposition: AlreadyMergedResolutionDisposition::AlreadyResolved,
                    goal,
                    evidence,
                });
            }
            AlreadyMergedInspection::AlreadyFailed(goal, evidence) => {
                return Ok(AlreadyMergedResolution {
                    disposition: AlreadyMergedResolutionDisposition::AlreadyFailed,
                    goal,
                    evidence,
                });
            }
            AlreadyMergedInspection::Eligible(snapshot) => snapshot,
        };
        if expected_authority.is_some_and(|authority| authority != snapshot.authority) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} already-merged resolver was superseded before repository inspection"
            )));
        }

        with_repository_git_lock(&target_root, || {
            let observation = if snapshot.gate_failure.is_none() {
                observe_integrated_candidate(&target_root, &self.runtime_root, &snapshot)
            } else {
                GitObservation::default()
            };
            let evidence = resolution_evidence(&snapshot, &observation);
            let decision = match snapshot
                .gate_failure
                .clone()
                .or_else(|| observation.error.clone())
            {
                Some(message) => AlreadyMergedSettlementDecision::Failed {
                    category: "already_merged_reconciliation_unsafe".to_string(),
                    message,
                    evidence,
                },
                None => AlreadyMergedSettlementDecision::Review(evidence),
            };
            map_settlement(
                work_items.settle_already_merged_resolution(goal_id, &snapshot, decision)?,
            )
        })
    }
}

fn observe_integrated_candidate(
    target_root: &Path,
    runtime_root: &Path,
    snapshot: &AlreadyMergedResolutionSnapshot,
) -> GitObservation {
    let mut observation = GitObservation::default();
    let Some(integration) = snapshot.integration.as_ref() else {
        observation.error = Some("current-Round integration evidence is unavailable".to_string());
        return observation;
    };
    let git = FileGitWorktreeService::with_runtime_root(target_root, runtime_root);
    let result = (|| -> RefineResult<()> {
        let resolved_candidate = git.resolve_commit(&snapshot.candidate_commit)?;
        if resolved_candidate != snapshot.candidate_commit {
            return Err(RefineError::Conflict(format!(
                "Candidate {} resolved as {resolved_candidate}",
                snapshot.candidate_commit
            )));
        }
        let resolved_integration = git.resolve_commit(&integration.target_commit)?;
        if resolved_integration != integration.target_commit {
            return Err(RefineError::Conflict(format!(
                "Integration target {} resolved as {resolved_integration}",
                integration.target_commit
            )));
        }
        if !git.commit_is_ancestor(&snapshot.candidate_commit, &integration.target_commit)? {
            return Err(RefineError::Conflict(format!(
                "Integration snapshot {} does not contain exact candidate {}",
                integration.target_commit, snapshot.candidate_commit
            )));
        }
        let local = git.resolve_commit(&integration.target_branch)?;
        observation.local_target_commit = Some(local.clone());
        if !git.commit_is_ancestor(&integration.target_commit, &local)?
            || !git.commit_is_ancestor(&snapshot.candidate_commit, &local)?
        {
            return Err(RefineError::Conflict(format!(
                "Local target {} at {local} no longer contains integration {} and candidate {}",
                integration.target_branch, integration.target_commit, snapshot.candidate_commit
            )));
        }
        if integration.pushed {
            git.fetch_branch(&integration.remote, &integration.target_branch)?;
            let published = git.resolve_commit(&format!(
                "{}/{}",
                integration.remote, integration.target_branch
            ))?;
            observation.published_target_commit = Some(published.clone());
            if !git.commit_is_ancestor(&integration.target_commit, &published)?
                || !git.commit_is_ancestor(&snapshot.candidate_commit, &published)?
            {
                return Err(RefineError::Conflict(format!(
                    "Published target {}/{} at {published} no longer contains integration {} and candidate {}",
                    integration.remote,
                    integration.target_branch,
                    integration.target_commit,
                    snapshot.candidate_commit
                )));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        observation.error = Some(format!(
            "Already-merged Git evidence is unsafe or unavailable: {error}"
        ));
    }
    observation
}

fn resolution_evidence(
    snapshot: &AlreadyMergedResolutionSnapshot,
    observation: &GitObservation,
) -> Value {
    let integration = snapshot.integration.as_ref();
    json!({
        "round_idx": snapshot.authority.round_idx,
        "workflow_revision": snapshot.authority.workflow_revision,
        "candidate_commit": snapshot.candidate_commit,
        "target_branch": integration.map(|value| value.target_branch.as_str()),
        "integration_target_commit": integration.map(|value| value.target_commit.as_str()),
        "local_target_commit": observation.local_target_commit,
        "published_target_commit": observation.published_target_commit,
        "gate_evidence": snapshot.gate_evidence,
        "error": snapshot.gate_failure.as_ref().or(observation.error.as_ref()),
        "observed_at": Utc::now().to_rfc3339()
    })
}

fn map_settlement(settlement: AlreadyMergedSettlement) -> RefineResult<AlreadyMergedResolution> {
    let (disposition, goal, evidence) = match settlement {
        AlreadyMergedSettlement::Resolved(goal, evidence) => {
            (AlreadyMergedResolutionDisposition::Resolved, goal, evidence)
        }
        AlreadyMergedSettlement::Failed(goal, evidence) => {
            (AlreadyMergedResolutionDisposition::Failed, goal, evidence)
        }
        AlreadyMergedSettlement::AlreadyResolved(goal, evidence) => (
            AlreadyMergedResolutionDisposition::AlreadyResolved,
            goal,
            evidence,
        ),
        AlreadyMergedSettlement::AlreadyFailed(goal, evidence) => (
            AlreadyMergedResolutionDisposition::AlreadyFailed,
            goal,
            evidence,
        ),
    };
    Ok(AlreadyMergedResolution {
        disposition,
        goal,
        evidence,
    })
}
