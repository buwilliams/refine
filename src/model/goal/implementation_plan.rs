use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::Timestamp;

pub const IMPLEMENTATION_PLAN_SCHEMA_VERSION: u32 = 1;

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
    /// Zero-based index, matching every other workflow/process fence.
    pub round_idx: usize,
    pub context_version: u64,
    pub context_digest: String,
    pub claim_id: String,
    pub execution_id: String,
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
pub struct PlanningProcessEvidence {
    pub operation_id: String,
    pub process_id: Option<String>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationChecklistItem {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub affected_behavior: Vec<String>,
    #[serde(default)]
    pub governance_rationale: Option<String>,
    #[serde(default)]
    pub verification: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CriticismResolution {
    pub criticism_id: String,
    pub resolution: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProposedImplementationPlan {
    pub summary: String,
    pub checklist: Vec<ImplementationChecklistItem>,
    #[serde(default)]
    pub criticism_resolutions: Vec<CriticismResolution>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationPlanArtifact {
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub process: PlanningProcessEvidence,
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
    #[serde(default)]
    pub findings: Vec<ImplementationCriticismFinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImplementationCriticismArtifact {
    pub started_at: Timestamp,
    pub completed_at: Timestamp,
    pub process: PlanningProcessEvidence,
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
    pub process_id: String,
    pub session_id: String,
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
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_before: Option<PlanningGitObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_after: Option<PlanningGitObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<PlanningProcessEvidence>,
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
    pub active_process: Option<PlanningProcessEvidence>,
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
}
