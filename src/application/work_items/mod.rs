mod service;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use service::{
    AlreadyMergedInspection, AlreadyMergedResolutionSnapshot, AlreadyMergedSettlement,
    AlreadyMergedSettlementDecision, GoalCancellationExpectation, WorkflowAttemptAuthority,
};
pub use service::{FileWorkItemService, WorkItemService, validate_manual_goal_transition};
pub use types::*;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureSettlement {
    AuthoritativeFailure,
    ExistingVerifiedOutcome,
    SupersededAttempt,
    UnpersistedEvidence(String),
}

pub use service::WorkflowControl;
