use std::fmt;

use thiserror::Error;

/// Classification of a structured-output failure, used to select repair and
/// reporting behavior without parsing message text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuredOutputErrorKind {
    Transport,
    Schema,
    Validation,
}

/// A typed structured-output failure carrying its contract label and precise
/// diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuredOutputError {
    Transport {
        label: String,
        detail: String,
    },
    Schema {
        label: String,
        path: Option<String>,
        detail: String,
    },
    Validation {
        label: String,
        detail: String,
    },
}

impl StructuredOutputError {
    pub fn transport(label: &str, detail: impl Into<String>) -> Self {
        Self::Transport {
            label: label.to_string(),
            detail: detail.into(),
        }
    }

    pub fn schema(label: &str, path: Option<String>, detail: impl Into<String>) -> Self {
        Self::Schema {
            label: label.to_string(),
            path,
            detail: detail.into(),
        }
    }

    pub fn validation(label: &str, detail: impl Into<String>) -> Self {
        Self::Validation {
            label: label.to_string(),
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> StructuredOutputErrorKind {
        match self {
            Self::Transport { .. } => StructuredOutputErrorKind::Transport,
            Self::Schema { .. } => StructuredOutputErrorKind::Schema,
            Self::Validation { .. } => StructuredOutputErrorKind::Validation,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Transport { label, .. }
            | Self::Schema { label, .. }
            | Self::Validation { label, .. } => label,
        }
    }
}

impl fmt::Display for StructuredOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { label, detail } => {
                write!(
                    formatter,
                    "agent returned invalid structured {label}: {detail}"
                )
            }
            Self::Schema {
                label,
                path,
                detail,
            } => {
                let location = match path {
                    Some(path) => format!("field `{path}`"),
                    None => "the root value".to_string(),
                };
                write!(
                    formatter,
                    "agent returned invalid structured {label}: does not match the required schema at {location}: {detail}"
                )
            }
            Self::Validation { detail, .. } => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for StructuredOutputError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateRecoveryConflictReason {
    StateMoved,
}

impl StateRecoveryConflictReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StateMoved => "state_moved",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeConflictStage {
    CandidateIntegration,
    TargetSynchronization,
}

impl MergeConflictStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CandidateIntegration => "candidate integration",
            Self::TargetSynchronization => "target synchronization",
        }
    }
}

#[derive(Debug, Error)]
pub enum RefineError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{message}")]
    StateRecoveryConflict {
        reason: StateRecoveryConflictReason,
        message: String,
    },
    #[error(
        "Candidate {candidate_commit} is stale: recorded base {recorded_base} is not its ancestor"
    )]
    StaleCandidate {
        candidate_commit: String,
        recorded_base: String,
        target_branch: String,
        target_commit: String,
    },
    /// A Git merge stopped on content conflicts. The conflicted paths stay
    /// structured so recovery machinery can retain them as Round evidence
    /// instead of flattening them into prose.
    #[error("{message}")]
    MergeConflict {
        stage: MergeConflictStage,
        conflicts: Vec<String>,
        message: String,
    },
    /// A compare-and-swap ref advance observed the target moving underneath
    /// it; the integration pass must refresh against the new tip and retry.
    #[error("target advanced: {reference} moved from {expected} to {current} before the update")]
    TargetAdvanced {
        reference: String,
        expected: String,
        current: String,
    },
    #[error("{0}")]
    QualityCandidateInfrastructure(Box<QualityCandidateInfrastructureError>),
    /// The host Git is older than the one state synchronization requires.
    /// Refine's three-way merge IS `git merge-tree`; there is no second
    /// implementation to fall back to, so this is a precondition of running at
    /// all on this node, and it is that NODE's condition — the rest of the
    /// fleet keeps converging over the state branch regardless.
    #[error(
        "Refine needs Git {required} or newer to synchronize state, but this node has {observed}. Upgrade Git on this node; every other node keeps syncing meanwhile."
    )]
    UnsupportedGitVersion { required: String, observed: String },
    #[error("{0}")]
    Degraded(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Serialization(String),
    #[error(transparent)]
    StructuredOutput(#[from] StructuredOutputError),
    #[error("{0}")]
    NotImplemented(String),
}

pub type RefineResult<T> = Result<T, RefineError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorCategory {
    InvalidInput,
    NotFound,
    Unauthorized,
    Conflict,
    Degraded,
    Io,
    Serialization,
    NotImplemented,
}

impl RefineError {
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::InvalidInput(_) => ErrorCategory::InvalidInput,
            Self::NotFound(_) => ErrorCategory::NotFound,
            Self::Unauthorized(_) => ErrorCategory::Unauthorized,
            Self::Conflict(_)
            | Self::StateRecoveryConflict { .. }
            | Self::MergeConflict { .. }
            | Self::StaleCandidate { .. }
            | Self::TargetAdvanced { .. }
            | Self::QualityCandidateInfrastructure(_) => ErrorCategory::Conflict,
            Self::Degraded(_) | Self::UnsupportedGitVersion { .. } => ErrorCategory::Degraded,
            Self::Io(_) => ErrorCategory::Io,
            Self::Serialization(_) | Self::StructuredOutput(_) => ErrorCategory::Serialization,
            Self::NotImplemented(_) => ErrorCategory::NotImplemented,
        }
    }
}

#[derive(Debug, Error)]
#[error(
    "Quality candidate infrastructure fault for Goal {goal_id} during {phase}: {reason}; expected round {expected_round_idx}, branch {expected_branch}, path {expected_path}, registered={expected_registered}, commit {expected_commit}; observed round {observed_round_idx:?}, branch {observed_branch:?}, path {observed_path:?}, registered={observed_registered}, commit {observed_commit:?}"
)]
pub struct QualityCandidateInfrastructureError {
    pub goal_id: String,
    pub phase: String,
    pub reason: String,
    pub expected_round_idx: usize,
    pub observed_round_idx: Option<usize>,
    pub expected_branch: String,
    pub observed_branch: Option<String>,
    pub expected_path: String,
    pub observed_path: Option<String>,
    pub expected_registered: bool,
    pub observed_registered: bool,
    pub expected_commit: String,
    pub observed_commit: Option<String>,
}
