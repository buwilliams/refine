use thiserror::Error;

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
            Self::Conflict(_) => ErrorCategory::Conflict,
            Self::Degraded(_) => ErrorCategory::Degraded,
            Self::Io(_) => ErrorCategory::Io,
            Self::Serialization(_) => ErrorCategory::Serialization,
            Self::NotImplemented(_) => ErrorCategory::NotImplemented,
        }
    }
}
