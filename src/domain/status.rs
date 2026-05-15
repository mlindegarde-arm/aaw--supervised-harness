use crate::error::{HarnessError, HarnessResult};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::parse(&String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
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

status_type!(ObjectiveStatus, "objective" {
    Planning => "planning",
    Ready => "ready",
    Running => "running",
    Blocked => "blocked",
    Complete => "complete",
    Failed => "failed",
    Cancelled => "cancelled",
});

status_type!(ObjectiveAcceptanceStatus, "objective_acceptance" {
    Pending => "pending",
    Passing => "passing",
    Failing => "failing",
    Waived => "waived",
});

status_type!(ObjectiveValidationReviewStatus, "objective_validation_review" {
    Trusted => "trusted",
    NeedsReview => "needs_review",
    Rejected => "rejected",
});

status_type!(ObjectiveValidationCommandSource, "objective_validation_command_source" {
    Planner => "planner",
    User => "user",
    System => "system",
});

status_type!(PlannerExchangeKind, "planner_exchange_kind" {
    InitialPlan => "initial_plan",
    TicketResolution => "ticket_resolution",
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

    #[test]
    fn objective_status_wire_names_parse_and_serialize() {
        assert_eq!(ObjectiveStatus::Planning.as_str(), "planning");
        assert_eq!(ObjectiveStatus::Ready.as_str(), "ready");
        assert_eq!(ObjectiveStatus::Running.as_str(), "running");
        assert_eq!(ObjectiveStatus::Blocked.as_str(), "blocked");
        assert_eq!(ObjectiveStatus::Complete.as_str(), "complete");
        assert_eq!(ObjectiveStatus::Failed.as_str(), "failed");
        assert_eq!(ObjectiveStatus::Cancelled.as_str(), "cancelled");

        let encoded = serde_json::to_string(&ObjectiveStatus::Blocked).unwrap();
        assert_eq!(encoded, "\"blocked\"");
        assert_eq!(
            serde_json::from_str::<ObjectiveStatus>("\"blocked\"").unwrap(),
            ObjectiveStatus::Blocked
        );
    }

    #[test]
    fn objective_status_rejects_unknown_values() {
        assert!("done".parse::<ObjectiveStatus>().is_err());
        assert!(serde_json::from_str::<ObjectiveStatus>("\"waiting\"").is_err());
    }

    #[test]
    fn objective_supporting_status_wire_names_are_stable() {
        assert_status_json_round_trip(ObjectiveAcceptanceStatus::Pending, "pending");
        assert_status_json_round_trip(ObjectiveAcceptanceStatus::Passing, "passing");
        assert_status_json_round_trip(ObjectiveAcceptanceStatus::Failing, "failing");
        assert_status_json_round_trip(ObjectiveAcceptanceStatus::Waived, "waived");
        assert_status_json_round_trip(ObjectiveValidationReviewStatus::Trusted, "trusted");
        assert_status_json_round_trip(ObjectiveValidationReviewStatus::NeedsReview, "needs_review");
        assert_status_json_round_trip(ObjectiveValidationReviewStatus::Rejected, "rejected");
        assert_status_json_round_trip(ObjectiveValidationCommandSource::Planner, "planner");
        assert_status_json_round_trip(ObjectiveValidationCommandSource::User, "user");
        assert_status_json_round_trip(ObjectiveValidationCommandSource::System, "system");
        assert_status_json_round_trip(PlannerExchangeKind::InitialPlan, "initial_plan");
        assert_status_json_round_trip(PlannerExchangeKind::TicketResolution, "ticket_resolution");
        assert!("replan".parse::<PlannerExchangeKind>().is_err());
    }

    fn assert_status_json_round_trip<T>(status: T, wire: &str)
    where
        T: std::str::FromStr<Err = HarnessError>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + std::fmt::Display
            + Copy
            + PartialEq
            + std::fmt::Debug,
    {
        assert_eq!(status.to_string(), wire);
        assert_eq!(wire.parse::<T>().unwrap(), status);

        let encoded = serde_json::to_string(&status).unwrap();
        assert_eq!(encoded, format!("\"{wire}\""));
        assert_eq!(serde_json::from_str::<T>(&encoded).unwrap(), status);
        assert!(serde_json::from_str::<T>("\"not_a_status\"").is_err());
    }
}
