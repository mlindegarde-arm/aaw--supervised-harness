use crate::domain::{ArtifactId, RunId, RunStatus, TaskId, TaskStatus, TicketId, TicketStatus};

use super::{CommandEvent, CommandExit, CommandResult};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperviseProgressPhase {
    InspectTask,
    RunTask,
    ResolveTicket,
    ResumeTask,
    Complete,
    Stuck,
    Failed,
    Cancelling,
    Cancelled,
}

impl SuperviseProgressPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InspectTask => "inspect",
            Self::RunTask => "run",
            Self::ResolveTicket => "resolve",
            Self::ResumeTask => "resume",
            Self::Complete => "complete",
            Self::Stuck => "stuck",
            Self::Failed => "failed",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuperviseProgressEvent {
    pub phase: SuperviseProgressPhase,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub ticket_id: Option<TicketId>,
    pub cycle: Option<u32>,
    pub message: String,
    pub next_command: Option<String>,
}

impl SuperviseProgressEvent {
    pub fn new(phase: SuperviseProgressPhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            task_id: None,
            run_id: None,
            ticket_id: None,
            cycle: None,
            message: message.into(),
            next_command: None,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "event": "supervise.phase",
            "phase": self.phase.as_str(),
            "task_id": self.task_id.as_ref().map(TaskId::as_str),
            "run_id": self.run_id.as_ref().map(RunId::as_str),
            "ticket_id": self.ticket_id.as_ref().map(TicketId::as_str),
            "cycle": self.cycle,
            "attempt": null,
            "status": self.phase.as_str(),
            "exit_code": null,
            "message": self.message,
            "artifact_paths": [],
            "elapsed_ms": 0,
            "next_command": self.next_command,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TuiRuntimeEvent {
    Stdout(String),
    Stderr(String),
    Progress(SuperviseProgressEvent),
    CommandEvent(CommandEvent),
    CommandFinished(CommandResult),
    CancelAcknowledged { next_command: Option<String> },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptEvent {
    Stdout(String),
    Stderr(String),
    Command(CommandEvent),
    SuperviseProgress(SuperviseProgressEvent),
    CommandFinished(CommandExit),
    CancellationAcknowledged { next_command: Option<String> },
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneStateSnapshot {
    pub tasks: PaneSection<PaneTaskRow>,
    pub tickets: PaneSection<PaneTicketRow>,
    pub runs: PaneSection<PaneRunRow>,
    pub artifacts: PaneSection<PaneArtifactRow>,
}

impl Default for PaneStateSnapshot {
    fn default() -> Self {
        Self {
            tasks: PaneSection::Loading,
            tickets: PaneSection::Loading,
            runs: PaneSection::Loading,
            artifacts: PaneSection::Loading,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneSection<T> {
    Loading,
    Ready(Vec<T>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneTaskRow {
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub title: String,
    pub latest_run_id: Option<RunId>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneTicketRow {
    pub ticket_id: TicketId,
    pub status: TicketStatus,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub blocked_on: String,
    pub question: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRunRow {
    pub run_id: RunId,
    pub task_id: TaskId,
    pub status: RunStatus,
    pub escalation_cycle: u32,
    pub current_phase: Option<String>,
    pub latest_artifact_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneArtifactRow {
    pub artifact_id: ArtifactId,
    pub kind: String,
    pub path: String,
    pub byte_len: u64,
    pub sha256_prefix: String,
    pub task_id: TaskId,
    pub run_id: Option<RunId>,
    pub ticket_id: Option<TicketId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn runtime_events_supervise_progress_event_is_structured() {
        let event = SuperviseProgressEvent {
            phase: SuperviseProgressPhase::ResolveTicket,
            task_id: Some(TaskId::parse(TASK_ID).unwrap()),
            run_id: Some(RunId::parse(RUN_ID).unwrap()),
            ticket_id: Some(TicketId::parse(TICKET_ID).unwrap()),
            cycle: Some(2),
            message: "resolving ticket".to_string(),
            next_command: Some(format!("harness supervise {TASK_ID} --output json")),
        };

        assert_eq!(event.phase, SuperviseProgressPhase::ResolveTicket);
        assert_eq!(event.task_id.as_ref().map(TaskId::as_str), Some(TASK_ID));
        assert_eq!(event.run_id.as_ref().map(RunId::as_str), Some(RUN_ID));
        assert_eq!(
            event.ticket_id.as_ref().map(TicketId::as_str),
            Some(TICKET_ID)
        );
        assert_eq!(event.cycle, Some(2));
        assert_eq!(
            event.next_command.as_deref(),
            Some("harness supervise task_01ARZ3NDEKTSV4RRFFQ69G5FAV --output json")
        );
    }

    #[test]
    fn runtime_events_pane_snapshot_has_tui_loading_contract() {
        let snapshot = PaneStateSnapshot::default();

        assert!(matches!(snapshot.tasks, PaneSection::Loading));
        assert!(matches!(snapshot.tickets, PaneSection::Loading));
        assert!(matches!(snapshot.runs, PaneSection::Loading));
        assert!(matches!(snapshot.artifacts, PaneSection::Loading));
    }

    #[test]
    fn runtime_events_transcript_wraps_supervisor_progress() {
        let progress =
            SuperviseProgressEvent::new(SuperviseProgressPhase::InspectTask, "inspecting task");
        let transcript = TranscriptEvent::SuperviseProgress(progress.clone());

        assert_eq!(transcript, TranscriptEvent::SuperviseProgress(progress));
    }
}
