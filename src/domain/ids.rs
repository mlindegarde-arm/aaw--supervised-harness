use crate::error::{HarnessError, HarnessResult};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{self, Display};
use std::str::FromStr;

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn parse(value: impl Into<String>) -> HarnessResult<Self> {
                let value = value.into();
                if value.strip_prefix(Self::PREFIX).is_some_and(is_valid_ulid) {
                    Ok(Self(value))
                } else {
                    Err(HarnessError::InvalidId {
                        expected_prefix: Self::PREFIX,
                        value,
                    })
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
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
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn is_valid_ulid(value: &str) -> bool {
    const ULID_LEN: usize = 26;
    const ULID_ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    value.len() == ULID_LEN
        && value
            .bytes()
            .next()
            .is_some_and(|first| matches!(first, b'0'..=b'7'))
        && value.chars().all(|ch| ULID_ALPHABET.contains(ch))
}

id_type!(TaskId, "task_");
id_type!(RunId, "run_");
id_type!(AttemptId, "att_");
id_type!(TicketId, "ticket_");
id_type!(TicketResolutionId, "res_");
id_type!(ArtifactId, "art_");
id_type!(EventId, "event_");
id_type!(ObjectiveId, "objective_");
id_type!(ObjectivePlanId, "plan_");
id_type!(ObjectiveEventId, "oevent_");
id_type!(ObjectiveArtifactId, "obj_art_");
id_type!(PlannerExchangeId, "planner_");
id_type!(ObjectiveMessageId, "omsg_");
id_type!(ObjectiveAcceptanceCriterionId, "criterion_");
id_type!(ObjectiveValidationCommandId, "validation_");
id_type!(ObjectiveResolverAttemptId, "resolver_");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_accept_their_expected_prefix() {
        assert_eq!(
            TaskId::parse("task_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "task_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            RunId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "run_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            AttemptId::parse("att_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "att_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            TicketId::parse("ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            TicketResolutionId::parse("res_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "res_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ArtifactId::parse("art_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "art_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            EventId::parse("event_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "event_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveId::parse("objective_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "objective_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectivePlanId::parse("plan_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "plan_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveEventId::parse("oevent_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "oevent_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveArtifactId::parse("obj_art_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "obj_art_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            PlannerExchangeId::parse("planner_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "planner_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveMessageId::parse("omsg_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "omsg_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveAcceptanceCriterionId::parse("criterion_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "criterion_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveValidationCommandId::parse("validation_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "validation_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
        assert_eq!(
            ObjectiveResolverAttemptId::parse("resolver_01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .unwrap()
                .as_str(),
            "resolver_01ARZ3NDEKTSV4RRFFQ69G5FAV"
        );
    }

    #[test]
    fn ids_reject_wrong_or_empty_prefix_payloads() {
        assert!(TaskId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
        assert!(TaskId::parse("task_").is_err());
        assert!(TaskId::parse("task_not-a-ulid").is_err());
        assert!(TaskId::parse("task_81ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
        assert!(TaskId::parse("task_01ARZ3NDEKTSV4RRFFQ69G5FAI").is_err());
        assert!(TaskId::parse("task_01arz3ndektsv4rrffq69g5fav").is_err());
    }

    #[test]
    fn objective_id_parse_display_and_json_round_trip() {
        assert_id_json_round_trip::<ObjectiveId>("objective_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_id_json_round_trip::<ObjectivePlanId>("plan_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_id_json_round_trip::<ObjectiveEventId>("oevent_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_id_json_round_trip::<ObjectiveArtifactId>("obj_art_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_id_json_round_trip::<PlannerExchangeId>("planner_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_id_json_round_trip::<ObjectiveMessageId>("omsg_01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_id_json_round_trip::<ObjectiveAcceptanceCriterionId>(
            "criterion_01ARZ3NDEKTSV4RRFFQ69G5FAV",
        );
        assert_id_json_round_trip::<ObjectiveValidationCommandId>(
            "validation_01ARZ3NDEKTSV4RRFFQ69G5FAV",
        );
        assert_id_json_round_trip::<ObjectiveResolverAttemptId>(
            "resolver_01ARZ3NDEKTSV4RRFFQ69G5FAV",
        );
    }

    fn assert_id_json_round_trip<T>(value: &str)
    where
        T: std::str::FromStr<Err = HarnessError>
            + serde::Serialize
            + serde::de::DeserializeOwned
            + std::fmt::Display
            + PartialEq
            + std::fmt::Debug,
    {
        let id = value.parse::<T>().unwrap();

        assert_eq!(id.to_string(), value);

        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, format!("\"{value}\""));
        let decoded: T = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, id);
        assert!(serde_json::from_str::<T>("\"bad_01ARZ3NDEKTSV4RRFFQ69G5FAV\"").is_err());
    }
}
