use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateRecoveryConflictReason {
    GitBusy,
    StalePreview,
}

impl StateRecoveryConflictReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitBusy => "git_busy",
            Self::StalePreview => "stale_preview",
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
    #[error("{0}")]
    StateSyncMissingBaseline(String),
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
    #[error("{0}")]
    QualityCandidateInfrastructure(Box<QualityCandidateInfrastructureError>),
    #[error("{0}")]
    Degraded(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Serialization(String),
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
            | Self::StateSyncMissingBaseline(_)
            | Self::StateRecoveryConflict { .. }
            | Self::StaleCandidate { .. }
            | Self::QualityCandidateInfrastructure(_) => ErrorCategory::Conflict,
            Self::Degraded(_) => ErrorCategory::Degraded,
            Self::Io(_) => ErrorCategory::Io,
            Self::Serialization(_) => ErrorCategory::Serialization,
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
