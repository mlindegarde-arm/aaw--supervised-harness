use std::error::Error;
use std::fmt::{self, Display};

pub type HarnessResult<T> = Result<T, HarnessError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessError {
    InvalidId {
        expected_prefix: &'static str,
        value: String,
    },
    InvalidStatus {
        kind: &'static str,
        value: String,
    },
    InvalidConfig(String),
    Usage(String),
    SecurityPolicy(String),
    NotFound {
        kind: &'static str,
        id: String,
    },
    Conflict(String),
    External(String),
}

impl Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId {
                expected_prefix,
                value,
            } => write!(
                f,
                "invalid id {value:?}; expected prefix {expected_prefix:?}"
            ),
            Self::InvalidStatus { kind, value } => {
                write!(f, "invalid {kind} status {value:?}")
            }
            Self::InvalidConfig(message) => write!(f, "invalid config: {message}"),
            Self::Usage(message) => write!(f, "{message}"),
            Self::SecurityPolicy(message) => {
                write!(f, "security policy blocked operation: {message}")
            }
            Self::NotFound { kind, id } => write!(f, "{kind} {id} was not found"),
            Self::Conflict(message) => write!(f, "conflict: {message}"),
            Self::External(message) => write!(f, "external command failed: {message}"),
        }
    }
}

impl Error for HarnessError {}
