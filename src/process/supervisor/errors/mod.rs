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
    #[error("{0}")]
    Degraded(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Serialization(String),
    #[error(transparent)]
    StructuredOutput(#[from] crate::structured_output::StructuredOutputError),
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
            | Self::MergeConflict { .. }
            | Self::StaleCandidate { .. }
            | Self::TargetAdvanced { .. }
            | Self::QualityCandidateInfrastructure(_) => ErrorCategory::Conflict,
            Self::Degraded(_) => ErrorCategory::Degraded,
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
