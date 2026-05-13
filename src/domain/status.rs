use crate::error::{HarnessError, HarnessResult};
use std::fmt::{self, Display};
use std::str::FromStr;

macro_rules! status_type {
    (
        $name:ident, $kind:literal {
            $($variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            pub fn parse(value: &str) -> HarnessResult<Self> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(HarnessError::InvalidStatus {
                        kind: $kind,
                        value: other.to_string(),
                    }),
                }
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = HarnessError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }
    };
}

status_type!(TaskStatus, "task" {
    Ready => "ready",
    Running => "running",
    Complete => "complete",
    Stuck => "stuck",
    Failed => "failed",
});

status_type!(RunStatus, "run" {
    Running => "running",
    Complete => "complete",
    Stuck => "stuck",
    Failed => "failed",
});

status_type!(AttemptStatus, "attempt" {
    Running => "running",
    Complete => "complete",
    Failed => "failed",
    InvalidResponse => "invalid_response",
    PatchRejected => "patch_rejected",
    ValidationFailed => "validation_failed",
});

status_type!(TicketStatus, "ticket" {
    Open => "open",
    Resolving => "resolving",
    Resolved => "resolved",
    Failed => "failed",
});

status_type!(EventLevel, "event_level" {
    Info => "info",
    Warn => "warn",
    Error => "error",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_wire_names_match_design() {
        assert_eq!(TaskStatus::Ready.as_str(), "ready");
        assert_eq!(TaskStatus::Running.as_str(), "running");
        assert_eq!(TaskStatus::Complete.as_str(), "complete");
        assert_eq!(TaskStatus::Stuck.as_str(), "stuck");
        assert_eq!(TaskStatus::Failed.as_str(), "failed");
        assert_eq!(TicketStatus::Resolving.as_str(), "resolving");
    }

    #[test]
    fn status_parse_rejects_unknown_values() {
        assert_eq!(
            "complete".parse::<TaskStatus>().unwrap(),
            TaskStatus::Complete
        );
        assert!("done".parse::<TaskStatus>().is_err());
    }
}
