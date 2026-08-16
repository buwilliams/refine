use std::fmt;

/// Classification of a structured-output failure, used to pick repair and
/// reporting behavior without parsing message text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuredOutputErrorKind {
    /// The output could not be reduced to one bounded JSON value: empty,
    /// oversized, over-nested, too many envelope or stringification layers,
    /// invalid JSON, no candidate, or ambiguous candidates.
    Transport,
    /// One JSON value was selected but does not match the serde schema.
    Schema,
    /// The value matched the schema but violated a semantic contract rule.
    Validation,
}

/// A typed structured-output failure carrying the contract label and the
/// precise diagnostic that repair prompts and persisted evidence rely on.
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { label, detail } => {
                write!(f, "agent returned invalid structured {label}: {detail}")
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
                    f,
                    "agent returned invalid structured {label}: does not match the required schema at {location}: {detail}"
                )
            }
            Self::Validation { detail, .. } => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for StructuredOutputError {}
