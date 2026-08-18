use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::application::work_items::FileWorkItemService;
use crate::error::{QualityCandidateInfrastructureError, RefineError, RefineResult};
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};

pub const ISOLATED_CANDIDATE: &str = "isolated_candidate";
pub const INTEGRATED_TARGET: &str = "integrated_target";
pub const INTEGRATED_TARGET_RECONCILIATION: &str = "integrated_target_reconciliation";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityIdentityCommitment {
    pub evaluation_scope: String,
    pub goal_id: String,
    pub round_idx: usize,
    pub branch: String,
    pub path: String,
    pub candidate_commit: String,
    pub source_candidate_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_commit: Option<String>,
}

impl QualityIdentityCommitment {
    pub fn isolated(
        goal_id: &str,
        round_idx: usize,
        branch: &str,
        path: &Path,
        candidate_commit: &str,
    ) -> Self {
        Self {
            evaluation_scope: ISOLATED_CANDIDATE.to_string(),
            goal_id: goal_id.to_string(),
            round_idx,
            branch: branch.to_string(),
            path: path.display().to_string(),
            candidate_commit: candidate_commit.to_string(),
            source_candidate_commit: candidate_commit.to_string(),
            target_branch: None,
            target_commit: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn integrated(
        scope: &str,
        goal_id: &str,
        round_idx: usize,
        target_branch: &str,
        target_root: &Path,
        target_commit: &str,
        source_candidate_commit: &str,
    ) -> Self {
        Self {
            evaluation_scope: scope.to_string(),
            goal_id: goal_id.to_string(),
            round_idx,
            branch: target_branch.to_string(),
            path: target_root.display().to_string(),
            candidate_commit: target_commit.to_string(),
            source_candidate_commit: source_candidate_commit.to_string(),
            target_branch: Some(target_branch.to_string()),
            target_commit: Some(target_commit.to_string()),
        }
    }
}

#[derive(Default)]
struct ObservedIdentity {
    round_idx: Option<usize>,
    branch: Option<String>,
    path: Option<String>,
    registered: bool,
    commit: Option<String>,
}

pub fn is_quality_candidate_infrastructure(error: &RefineError) -> bool {
    matches!(error, RefineError::QualityCandidateInfrastructure(_))
}

pub fn validate_quality_identity(
    refine_dir: &Path,
    target_root: &Path,
    runtime_root: &Path,
    commitment: &QualityIdentityCommitment,
    phase: &str,
) -> RefineResult<()> {
    let work_items = FileWorkItemService::new(refine_dir);
    let summary = work_items.show_goal_summary(&commitment.goal_id)?;
    let detail = work_items.show_goal_detail(&commitment.goal_id)?;
    let git = FileGitWorktreeService::with_runtime_root(target_root, runtime_root);
    let mut observed = ObservedIdentity {
        round_idx: summary.goal.round_count.checked_sub(1),
        branch: summary.goal.branch_name.clone(),
        ..ObservedIdentity::default()
    };

    if observed.round_idx != Some(commitment.round_idx) {
        return Err(infrastructure_error(
            commitment,
            phase,
            "the current Goal Round changed",
            &observed,
        ));
    }

    let round = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(commitment.round_idx))
        .ok_or_else(|| {
            infrastructure_error(
                commitment,
                phase,
                "the committed Round is absent",
                &observed,
            )
        })?;

    if commitment.evaluation_scope == ISOLATED_CANDIDATE {
        let reconciliation_checkout_matches = round
            .get("workflow_reconciliation")
            .and_then(|evidence| evidence.get("quality_checkout"))
            .is_some_and(|checkout| {
                checkout.get("branch").and_then(Value::as_str) == Some(commitment.branch.as_str())
                    && checkout.get("path").and_then(Value::as_str)
                        == Some(commitment.path.as_str())
                    && checkout.get("candidate_commit").and_then(Value::as_str)
                        == Some(commitment.candidate_commit.as_str())
                    && round
                        .get("workflow_integration")
                        .and_then(|integration| integration.get("candidate_commit"))
                        .and_then(Value::as_str)
                        == Some(commitment.source_candidate_commit.as_str())
            });
        if observed.branch.as_deref() != Some(commitment.branch.as_str())
            && !reconciliation_checkout_matches
        {
            return Err(infrastructure_error(
                commitment,
                phase,
                "the candidate branch changed",
                &observed,
            ));
        }
        let registered = git.existing_worktree_for_branch(&commitment.branch)?;
        observed.registered = registered.is_some();
        observed.path = registered.as_ref().map(|path| path.display().to_string());
        if observed.path.as_deref() != Some(commitment.path.as_str()) {
            return Err(infrastructure_error(
                commitment,
                phase,
                "the exact candidate worktree is not registered",
                &observed,
            ));
        }
    } else {
        let integration = round
            .get("workflow_integration")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                infrastructure_error(
                    commitment,
                    phase,
                    "integrated-target Quality has no integration evidence",
                    &observed,
                )
            })?;
        if integration.get("candidate_commit").and_then(Value::as_str)
            != Some(commitment.source_candidate_commit.as_str())
            || integration.get("target_branch").and_then(Value::as_str)
                != commitment.target_branch.as_deref()
            || integration.get("target_commit").and_then(Value::as_str)
                != commitment.target_commit.as_deref()
        {
            return Err(infrastructure_error(
                commitment,
                phase,
                "integration evidence changed",
                &observed,
            ));
        }
        observed.branch = git.head_ref()?.branch;
        observed.path = Some(target_root.display().to_string());
        observed.registered = observed.branch.as_deref() == Some(commitment.branch.as_str());
    }

    let committed_path = PathBuf::from(&commitment.path);
    if !committed_path.is_dir() {
        return Err(infrastructure_error(
            commitment,
            phase,
            "the committed checkout path does not exist",
            &observed,
        ));
    }
    let committed_git = FileGitWorktreeService::with_runtime_root(&committed_path, runtime_root);
    let head = committed_git.head_ref()?;
    observed.commit = head.commit;
    if head.branch.as_deref() != Some(commitment.branch.as_str())
        || observed.commit.as_deref() != Some(commitment.candidate_commit.as_str())
    {
        observed.branch = head.branch;
        return Err(infrastructure_error(
            commitment,
            phase,
            "the committed checkout branch or HEAD changed",
            &observed,
        ));
    }
    let status = committed_git.inspect("")?;
    if !status.is_pristine() {
        return Err(infrastructure_error(
            commitment,
            phase,
            "the committed checkout is dirty",
            &observed,
        ));
    }
    Ok(())
}

fn infrastructure_error(
    commitment: &QualityIdentityCommitment,
    phase: &str,
    reason: &str,
    observed: &ObservedIdentity,
) -> RefineError {
    RefineError::QualityCandidateInfrastructure(Box::new(QualityCandidateInfrastructureError {
        goal_id: commitment.goal_id.clone(),
        phase: phase.to_string(),
        reason: reason.to_string(),
        expected_round_idx: commitment.round_idx,
        observed_round_idx: observed.round_idx,
        expected_branch: commitment.branch.clone(),
        observed_branch: observed.branch.clone(),
        expected_path: commitment.path.clone(),
        observed_path: observed.path.clone(),
        expected_registered: true,
        observed_registered: observed.registered,
        expected_commit: commitment.candidate_commit.clone(),
        observed_commit: observed.commit.clone(),
    }))
}
