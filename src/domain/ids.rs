use crate::error::{HarnessError, HarnessResult};
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
}
