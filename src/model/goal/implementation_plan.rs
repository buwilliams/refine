use serde::{Deserialize, Serialize};

use crate::model::Timestamp;

pub const IMPLEMENTATION_PLAN_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationPlanPhase {
    Plan,
    Criticize,
    Revise,
    Implement,
}

impl ImplementationPlanPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Criticize => "criticize",
            Self::Revise => "revise",
            Self::Implement => "implement",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationPlanState {
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationPlanBinding {
    pub goal_id: String,
    /// Zero-based index of the synchronized Goal Round.
    pub round_idx: usize,
    pub context_version: u64,
    pub context_digest: String,
    pub implementation_branch: String,
    pub target_branch: String,
    pub base_commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanningGitObservation {
    pub head_commit: String,
    pub branch: Option<String>,
    pub status_porcelain: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationChecklistItem {
    pub id: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_behavior: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance_rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CriticismResolution {
    pub criticism_id: String,
    pub resolution: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProposedImplementationPlan {
    pub summary: String,
    pub checklist: Vec<ImplementationChecklistItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criticism_resolutions: Vec<CriticismResolution>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationPlanArtifact {
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub git_before: PlanningGitObservation,
    pub git_after: PlanningGitObservation,
    pub result: ProposedImplementationPlan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationCriticismFinding {
    pub id: String,
    pub material: bool,
    #[serde(default)]
    pub checklist_item_ids: Vec<String>,
    pub description: String,
    pub recommendation: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationCriticism {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<ImplementationCriticismFinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationCriticismArtifact {
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub git_before: PlanningGitObservation,
    pub git_after: PlanningGitObservation,
    pub result: ImplementationCriticism,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationChecklistOutcome {
    Completed,
    Deviated,
    Rejected,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationChecklistResult {
    pub id: String,
    pub outcome: ImplementationChecklistOutcome,
    pub evidence: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationExecutionEvidence {
    #[serde(default)]
    pub checklist: Vec<ImplementationChecklistResult>,
    #[serde(default)]
    pub verification: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationAgentEvidence {
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub report: String,
    pub execution: ImplementationExecutionEvidence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationPlanningFailure {
    pub phase: ImplementationPlanPhase,
    pub category: String,
    pub message: String,
    pub failed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_before: Option<PlanningGitObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_after: Option<PlanningGitObservation>,
}

/// One invalid structured response retained before a bounded diagnostic repair invocation.
///
/// Keeping this evidence on the Round makes provider/output-contract failures inspectable even
/// when a later repair succeeds. `attempt` is one-based within the current planning phase.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationPlanningOutputAttempt {
    pub phase: ImplementationPlanPhase,
    pub attempt: usize,
    pub raw_output: String,
    pub diagnostics: String,
    pub recorded_at: Timestamp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationPlan {
    pub schema_version: u32,
    pub state: ImplementationPlanState,
    pub phase: ImplementationPlanPhase,
    pub binding: ImplementationPlanBinding,
    pub started_at: Timestamp,
    pub phase_started_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ImplementationPlanArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criticism: Option<ImplementationCriticismArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_plan: Option<ImplementationPlanArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<ImplementationAgentEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ImplementationPlanningFailure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_output_attempts: Vec<ImplementationPlanningOutputAttempt>,
}
