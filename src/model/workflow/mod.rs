use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    AwaitingRebuild,
    Review,
    Done,
    Failed,
    Cancelled,
}

impl GapStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in-progress",
            Self::Qa => "qa",
            Self::ReadyMerge => "ready-merge",
            Self::AwaitingRebuild => "awaiting-rebuild",
            Self::Review => "review",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "backlog" => Some(Self::Backlog),
            "todo" => Some(Self::Todo),
            "in-progress" => Some(Self::InProgress),
            "qa" => Some(Self::Qa),
            "ready-merge" => Some(Self::ReadyMerge),
            "awaiting-rebuild" => Some(Self::AwaitingRebuild),
            "review" => Some(Self::Review),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalGapStatus {
    Done,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutomatedGapStatus {
    InProgress,
    Qa,
    ReadyMerge,
    AwaitingRebuild,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BulkStatusTarget {
    Backlog,
    Todo,
    AwaitingRebuild,
    Review,
    Done,
    Failed,
    Cancelled,
    #[serde(rename = "__last_workflow_state")]
    RestoreLastWorkflowState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeatureWorkflowTarget {
    Backlog,
    Todo,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeatureProtectedStatus {
    Review,
    Done,
    ReadyMerge,
    AwaitingRebuild,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeatureCancelStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    AwaitingRebuild,
    Review,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapOperation {
    CreateGap,
    EditMetadata,
    EditNotes,
    SubmitNewRound,
    EditLatestRound,
    StartImplementation,
    CancelAutomation,
    RetryAgent,
    RetryQa,
    RetryMerge,
    VerifyReview,
    Merge,
    Undo,
    Delete,
    AssignToFeature,
    RemoveFromFeature,
    ReorderInFeature,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureOperation {
    CreateFeature,
    EditMetadata,
    AddGap,
    RemoveGap,
    ReorderGap,
    MoveWorkflow,
    CancelFeature,
    DeleteFeature,
    Import,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransitionDecision {
    pub allowed: bool,
    pub no_op: bool,
    pub reason: Option<String>,
}

impl TransitionDecision {
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            no_op: false,
            reason: None,
        }
    }

    pub fn no_op() -> Self {
        Self {
            allowed: true,
            no_op: true,
            reason: None,
        }
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            no_op: false,
            reason: Some(reason.into()),
        }
    }
}

pub fn user_status_transition(from: &GapStatus, to: &GapStatus) -> TransitionDecision {
    use GapStatus::*;

    if from == to {
        return TransitionDecision::no_op();
    }

    let allowed = matches!(
        (from, to),
        (Backlog, Todo)
            | (Todo, Backlog)
            | (Review, Todo)
            | (Done, Review)
            | (Failed, Todo)
            | (Cancelled, Todo)
    );

    if allowed {
        TransitionDecision::allowed()
    } else {
        TransitionDecision::denied(format!(
            "manual transition from {from:?} to {to:?} is not allowed"
        ))
    }
}

pub fn is_bulk_target_allowed(status: &GapStatus) -> bool {
    !matches!(
        status,
        GapStatus::InProgress | GapStatus::Qa | GapStatus::ReadyMerge
    )
}

pub fn is_automated_status(status: &GapStatus) -> bool {
    matches!(
        status,
        GapStatus::InProgress | GapStatus::Qa | GapStatus::ReadyMerge | GapStatus::AwaitingRebuild
    )
}

pub fn is_terminal_status(status: &GapStatus) -> bool {
    matches!(status, GapStatus::Done | GapStatus::Cancelled)
}

pub fn is_feature_protected_status(status: &GapStatus) -> bool {
    matches!(
        status,
        GapStatus::Review | GapStatus::Done | GapStatus::ReadyMerge | GapStatus::AwaitingRebuild
    )
}

pub fn is_feature_cancel_status(status: &GapStatus) -> bool {
    matches!(
        status,
        GapStatus::Backlog
            | GapStatus::Todo
            | GapStatus::InProgress
            | GapStatus::Qa
            | GapStatus::ReadyMerge
            | GapStatus::AwaitingRebuild
            | GapStatus::Review
            | GapStatus::Failed
    )
}

pub fn gap_operation_allowed(status: &GapStatus, operation: &GapOperation) -> TransitionDecision {
    use GapOperation::*;
    use GapStatus::*;

    let allowed = match operation {
        CreateGap => true,
        EditMetadata | EditNotes | Delete => !matches!(status, InProgress | Qa | ReadyMerge),
        SubmitNewRound => matches!(status, Todo | Failed | Review | Backlog),
        EditLatestRound => matches!(status, Backlog | Todo | Review),
        StartImplementation => matches!(status, Todo),
        CancelAutomation => is_automated_status(status),
        RetryAgent => matches!(status, Failed | InProgress),
        RetryQa => matches!(status, Qa | Failed),
        RetryMerge => matches!(status, ReadyMerge | Failed),
        VerifyReview => matches!(status, Review | Qa),
        Merge => matches!(status, ReadyMerge),
        Undo => matches!(status, Done | Review | Cancelled),
        AssignToFeature | RemoveFromFeature | ReorderInFeature => {
            !matches!(status, InProgress | Qa | ReadyMerge)
        }
    };

    if allowed {
        TransitionDecision::allowed()
    } else {
        TransitionDecision::denied(format!(
            "operation {operation:?} is not allowed for {status:?}"
        ))
    }
}

pub fn feature_operation_allowed(
    gap_statuses: &[GapStatus],
    operation: &FeatureOperation,
) -> TransitionDecision {
    use FeatureOperation::*;

    let has_active_automation = gap_statuses.iter().any(is_automated_status);
    let has_protected = gap_statuses.iter().any(is_feature_protected_status);

    let allowed = match operation {
        CreateFeature | EditMetadata | Import | DeleteFeature => true,
        AddGap | RemoveGap | ReorderGap | MoveWorkflow => !has_protected && !has_active_automation,
        CancelFeature => gap_statuses.iter().any(is_feature_cancel_status),
    };

    if allowed {
        TransitionDecision::allowed()
    } else {
        TransitionDecision::denied(format!("feature operation {operation:?} is not allowed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_user_transitions_match_spec() {
        assert!(user_status_transition(&GapStatus::Backlog, &GapStatus::Todo).allowed);
        assert!(user_status_transition(&GapStatus::Todo, &GapStatus::Backlog).allowed);
        assert!(user_status_transition(&GapStatus::Review, &GapStatus::Todo).allowed);
        assert!(user_status_transition(&GapStatus::Done, &GapStatus::Review).allowed);
        assert!(user_status_transition(&GapStatus::Failed, &GapStatus::Todo).allowed);
        assert!(user_status_transition(&GapStatus::Cancelled, &GapStatus::Todo).allowed);
        assert!(user_status_transition(&GapStatus::Qa, &GapStatus::Qa).no_op);
        assert!(!user_status_transition(&GapStatus::Todo, &GapStatus::ReadyMerge).allowed);
        assert!(!user_status_transition(&GapStatus::Backlog, &GapStatus::InProgress).allowed);
    }

    #[test]
    fn bulk_targets_exclude_system_owned_states() {
        assert!(!is_bulk_target_allowed(&GapStatus::InProgress));
        assert!(!is_bulk_target_allowed(&GapStatus::Qa));
        assert!(!is_bulk_target_allowed(&GapStatus::ReadyMerge));
        assert!(is_bulk_target_allowed(&GapStatus::AwaitingRebuild));
        assert!(is_bulk_target_allowed(&GapStatus::Done));
    }

    #[test]
    fn feature_rules_protect_specified_gap_statuses() {
        assert!(is_feature_protected_status(&GapStatus::Review));
        assert!(is_feature_protected_status(&GapStatus::Done));
        assert!(is_feature_protected_status(&GapStatus::ReadyMerge));
        assert!(is_feature_protected_status(&GapStatus::AwaitingRebuild));
        assert!(!is_feature_protected_status(&GapStatus::Failed));
    }
}
