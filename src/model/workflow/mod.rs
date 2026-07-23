use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GoalStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    Build,
    Review,
    Done,
    Failed,
    Cancelled,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in-progress",
            Self::Qa => "qa",
            Self::ReadyMerge => "ready-merge",
            Self::Build => "build",
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
            "build" => Some(Self::Build),
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
pub enum TerminalGoalStatus {
    Done,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutomatedGoalStatus {
    InProgress,
    Qa,
    ReadyMerge,
    Build,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BulkStatusTarget {
    Backlog,
    Todo,
    Build,
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
    Build,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeatureCancelStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    Build,
    Review,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalOperation {
    CreateGoal,
    EditMetadata,
    EditNotes,
    SubmitNewRound,
    EditLatestRound,
    StartImplementation,
    CancelAutomation,
    RetryAgent,
    RetryQa,
    RetryMerge,
    SubmitMerge,
    VerifyReview,
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
    AddGoal,
    RemoveGoal,
    ReorderGoal,
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

pub fn user_status_transition(from: &GoalStatus, to: &GoalStatus) -> TransitionDecision {
    use GoalStatus::*;

    if from == to {
        return TransitionDecision::no_op();
    }

    let allowed = matches!(
        (from, to),
        (Backlog, Todo) | (Todo, Backlog) | (Done, Review) | (Failed, Todo) | (Cancelled, Todo)
    );

    if allowed {
        TransitionDecision::allowed()
    } else {
        TransitionDecision::denied(format!(
            "manual transition from {from:?} to {to:?} is not allowed"
        ))
    }
}

pub fn is_bulk_target_allowed(status: &GoalStatus) -> bool {
    !matches!(
        status,
        GoalStatus::InProgress
            | GoalStatus::Qa
            | GoalStatus::ReadyMerge
            | GoalStatus::Build
            | GoalStatus::Review
            | GoalStatus::Done
    )
}

pub fn is_automated_status(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::InProgress | GoalStatus::Qa | GoalStatus::ReadyMerge | GoalStatus::Build
    )
}

pub fn is_terminal_status(status: &GoalStatus) -> bool {
    matches!(status, GoalStatus::Done | GoalStatus::Cancelled)
}

pub fn is_feature_protected_status(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::Review | GoalStatus::Done | GoalStatus::ReadyMerge | GoalStatus::Build
    )
}

pub fn is_feature_cancel_status(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::Backlog
            | GoalStatus::Todo
            | GoalStatus::InProgress
            | GoalStatus::Qa
            | GoalStatus::ReadyMerge
            | GoalStatus::Build
            | GoalStatus::Review
            | GoalStatus::Failed
    )
}

pub fn goal_operation_allowed(
    status: &GoalStatus,
    operation: &GoalOperation,
) -> TransitionDecision {
    use GoalOperation::*;
    use GoalStatus::*;

    let allowed = match operation {
        CreateGoal => true,
        EditMetadata | EditNotes | Delete => !matches!(status, InProgress | Qa | ReadyMerge),
        SubmitNewRound => matches!(status, Todo | Failed | Review | Backlog),
        EditLatestRound => matches!(status, Backlog | Todo | Review),
        StartImplementation => matches!(status, Todo),
        CancelAutomation => is_automated_status(status),
        RetryAgent => matches!(status, Failed | InProgress),
        RetryQa => matches!(status, Qa | Failed),
        RetryMerge => matches!(status, ReadyMerge | Failed),
        SubmitMerge => matches!(status, ReadyMerge),
        VerifyReview => matches!(status, Review),
        Undo => matches!(status, Done | Cancelled),
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
    goal_statuses: &[GoalStatus],
    operation: &FeatureOperation,
) -> TransitionDecision {
    use FeatureOperation::*;

    let has_active_automation = goal_statuses.iter().any(is_automated_status);
    let has_protected = goal_statuses.iter().any(is_feature_protected_status);

    let allowed = match operation {
        CreateFeature | EditMetadata | Import | DeleteFeature => true,
        AddGoal | RemoveGoal | ReorderGoal | MoveWorkflow => {
            !has_protected && !has_active_automation
        }
        CancelFeature => goal_statuses.iter().any(is_feature_cancel_status),
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
        assert!(user_status_transition(&GoalStatus::Backlog, &GoalStatus::Todo).allowed);
        assert!(user_status_transition(&GoalStatus::Todo, &GoalStatus::Backlog).allowed);
        assert!(!user_status_transition(&GoalStatus::Review, &GoalStatus::Todo).allowed);
        assert!(user_status_transition(&GoalStatus::Done, &GoalStatus::Review).allowed);
        assert!(user_status_transition(&GoalStatus::Failed, &GoalStatus::Todo).allowed);
        assert!(user_status_transition(&GoalStatus::Cancelled, &GoalStatus::Todo).allowed);
        assert!(user_status_transition(&GoalStatus::Qa, &GoalStatus::Qa).no_op);
        assert!(!user_status_transition(&GoalStatus::Todo, &GoalStatus::ReadyMerge).allowed);
        assert!(!user_status_transition(&GoalStatus::Backlog, &GoalStatus::InProgress).allowed);
    }

    #[test]
    fn bulk_targets_exclude_system_owned_states() {
        assert!(!is_bulk_target_allowed(&GoalStatus::InProgress));
        assert!(!is_bulk_target_allowed(&GoalStatus::Qa));
        assert!(!is_bulk_target_allowed(&GoalStatus::ReadyMerge));
        assert!(!is_bulk_target_allowed(&GoalStatus::Build));
        assert!(!is_bulk_target_allowed(&GoalStatus::Review));
        assert!(!is_bulk_target_allowed(&GoalStatus::Done));
        assert!(is_bulk_target_allowed(&GoalStatus::Todo));
    }

    #[test]
    fn feature_rules_protect_specified_goal_statuses() {
        assert!(is_feature_protected_status(&GoalStatus::Review));
        assert!(is_feature_protected_status(&GoalStatus::Done));
        assert!(is_feature_protected_status(&GoalStatus::ReadyMerge));
        assert!(is_feature_protected_status(&GoalStatus::Build));
        assert!(!is_feature_protected_status(&GoalStatus::Failed));
    }
}
