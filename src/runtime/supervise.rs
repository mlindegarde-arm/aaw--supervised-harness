use crate::domain::TicketId;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::RuntimeOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuperviseTaskOptions {
    pub runtime: RuntimeOptions,
    pub ticket_id: Option<TicketId>,
    pub max_attempts: Option<u32>,
    pub model: Option<String>,
    pub ticket_model: Option<String>,
    pub max_cycles: Option<u32>,
}

impl Default for SuperviseTaskOptions {
    fn default() -> Self {
        Self {
            runtime: RuntimeOptions::default(),
            ticket_id: None,
            max_attempts: None,
            model: None,
            ticket_model: None,
            max_cycles: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuperviseCreateOptions {
    pub runtime: RuntimeOptions,
    pub title: String,
    pub goal: String,
    pub validation_commands: Vec<String>,
    pub max_attempts: Option<u32>,
    pub model: Option<String>,
    pub ticket_model: Option<String>,
    pub max_cycles: Option<u32>,
}

impl SuperviseCreateOptions {
    pub fn new(
        title: impl Into<String>,
        goal: impl Into<String>,
        validation_commands: Vec<String>,
    ) -> Self {
        Self {
            runtime: RuntimeOptions::default(),
            title: title.into(),
            goal: goal.into(),
            validation_commands,
            max_attempts: None,
            model: None,
            ticket_model: None,
            max_cycles: None,
        }
    }
}

pub trait CancellationToken: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

#[derive(Debug, Clone, Default)]
pub struct CooperativeCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CooperativeCancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl CancellationToken for CooperativeCancellationToken {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl CancellationToken for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::OutputMode;
    use std::path::PathBuf;

    #[test]
    fn runtime_supervise_options_preserve_cli_contract_fields() {
        let options = SuperviseTaskOptions {
            runtime: RuntimeOptions {
                output: OutputMode::Json,
                quiet: true,
                repo: Some(PathBuf::from("/repo")),
                state_dir: Some(PathBuf::from("/state")),
            },
            ticket_id: Some(TicketId::parse("ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap()),
            max_attempts: Some(2),
            model: Some("local-model".to_string()),
            ticket_model: Some("ticket-model".to_string()),
            max_cycles: Some(3),
        };

        assert_eq!(options.runtime.output, OutputMode::Json);
        assert!(options.runtime.quiet);
        assert_eq!(options.runtime.repo, Some(PathBuf::from("/repo")));
        assert_eq!(
            options.ticket_id.as_ref().map(TicketId::as_str),
            Some("ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV")
        );
        assert_eq!(options.max_attempts, Some(2));
        assert_eq!(options.model.as_deref(), Some("local-model"));
        assert_eq!(options.ticket_model.as_deref(), Some("ticket-model"));
        assert_eq!(options.max_cycles, Some(3));
    }

    #[test]
    fn runtime_supervise_create_options_have_explicit_task_inputs() {
        let options = SuperviseCreateOptions::new(
            "Fix tests",
            "Make cargo test pass",
            vec!["cargo test".to_string()],
        );

        assert_eq!(options.title, "Fix tests");
        assert_eq!(options.goal, "Make cargo test pass");
        assert_eq!(options.validation_commands, ["cargo test"]);
        assert_eq!(options.runtime.output, OutputMode::Human);
    }

    #[test]
    fn runtime_supervise_cancellation_token_is_cooperative() {
        let token = CooperativeCancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }
}
