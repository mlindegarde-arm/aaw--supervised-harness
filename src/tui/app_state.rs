use crate::runtime::{CommandEvent, PaneStateSnapshot, TranscriptEvent, TuiRuntimeEvent};
use crate::tui::composer::SuggestionRow;
use crate::tui::panes::{DashboardPaneSnapshot, SidePaneState};
use crate::tui::transcript::TranscriptState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiFocus {
    Composer,
    Transcript,
    SidePane,
    HelpOverlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandActivity {
    Idle,
    Running,
    Cancelling,
}

impl CommandActivity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderState {
    pub repo: String,
    pub local_model: String,
    pub ticket_model: String,
    pub active_task: Option<String>,
    pub active_run: Option<String>,
    pub open_tickets: usize,
    pub phase: String,
}

impl Default for HeaderState {
    fn default() -> Self {
        Self {
            repo: "-".to_string(),
            local_model: "-".to_string(),
            ticket_model: "-".to_string(),
            active_task: None,
            active_run: None,
            open_tickets: 0,
            phase: "idle".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterState {
    pub mode: String,
    pub status: String,
    pub hints: Vec<String>,
}

impl Default for FooterState {
    fn default() -> Self {
        Self {
            mode: "composer".to_string(),
            status: "idle".to_string(),
            hints: vec![
                "Tab complete".to_string(),
                "Ctrl-P/N pane".to_string(),
                "? help".to_string(),
                "Ctrl-D exit".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerViewState {
    pub prompt: String,
    pub text: String,
    pub cursor: usize,
    pub disabled: bool,
    pub suggestions: Vec<SuggestionRow>,
    pub hint: Option<String>,
}

impl Default for ComposerViewState {
    fn default() -> Self {
        Self {
            prompt: "> ".to_string(),
            text: String::new(),
            cursor: 0,
            disabled: false,
            suggestions: Vec::new(),
            hint: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TuiAppState {
    pub header: HeaderState,
    pub footer: FooterState,
    pub composer: ComposerViewState,
    pub transcript: TranscriptState,
    pub panes: SidePaneState,
    pub focus: TuiFocus,
    pub activity: CommandActivity,
}

impl Default for TuiAppState {
    fn default() -> Self {
        Self {
            header: HeaderState::default(),
            footer: FooterState::default(),
            composer: ComposerViewState::default(),
            transcript: TranscriptState::default(),
            panes: SidePaneState::default(),
            focus: TuiFocus::Composer,
            activity: CommandActivity::Idle,
        }
    }
}

impl TuiAppState {
    pub fn set_pane_snapshot(&mut self, snapshot: PaneStateSnapshot) {
        self.header.open_tickets = match &snapshot.tickets {
            crate::runtime::PaneSection::Ready(rows) => rows
                .iter()
                .filter(|row| row.status == crate::domain::TicketStatus::Open)
                .count(),
            crate::runtime::PaneSection::Loading | crate::runtime::PaneSection::Error(_) => {
                self.header.open_tickets
            }
        };
        self.panes.set_snapshot(snapshot);
    }

    pub fn set_dashboard_snapshot(&mut self, snapshot: DashboardPaneSnapshot) {
        self.header.open_tickets = match &snapshot.phase2.tickets {
            crate::runtime::PaneSection::Ready(rows) => rows
                .iter()
                .filter(|row| row.status == crate::domain::TicketStatus::Open)
                .count(),
            crate::runtime::PaneSection::Loading | crate::runtime::PaneSection::Error(_) => {
                self.header.open_tickets
            }
        };
        self.panes.set_dashboard_snapshot(snapshot);
    }

    pub fn set_activity(&mut self, activity: CommandActivity) {
        self.activity = activity;
        self.composer.disabled = activity != CommandActivity::Idle;
        self.footer.status = activity.label().to_string();
        self.header.phase = activity.label().to_string();
    }

    pub fn append_transcript_event(&mut self, event: TranscriptEvent) {
        self.transcript.append_event(event);
    }

    pub fn append_runtime_event(&mut self, event: TuiRuntimeEvent) {
        match event {
            TuiRuntimeEvent::Stdout(text) => {
                self.append_transcript_event(TranscriptEvent::Stdout(text));
            }
            TuiRuntimeEvent::Stderr(text) => {
                self.append_transcript_event(TranscriptEvent::Stderr(text));
            }
            TuiRuntimeEvent::Progress(progress) => {
                self.header.phase = format!("{:?}", progress.phase);
                self.header.active_task = progress.task_id.as_ref().map(ToString::to_string);
                self.header.active_run = progress.run_id.as_ref().map(ToString::to_string);
                self.append_transcript_event(TranscriptEvent::SuperviseProgress(progress));
            }
            TuiRuntimeEvent::CommandEvent(event) => {
                self.apply_command_event(&event);
                self.append_transcript_event(TranscriptEvent::Command(event));
            }
            TuiRuntimeEvent::CommandFinished(result) => {
                self.set_activity(CommandActivity::Idle);
                for event in result.events {
                    self.apply_command_event(&event);
                    self.append_transcript_event(TranscriptEvent::Command(event));
                }
                self.append_transcript_event(TranscriptEvent::CommandFinished(result.exit));
                self.panes.mark_stale();
            }
            TuiRuntimeEvent::CancelAcknowledged { next_command } => {
                self.set_activity(CommandActivity::Idle);
                self.append_transcript_event(TranscriptEvent::CancellationAcknowledged {
                    next_command,
                });
                self.panes.mark_stale();
            }
            TuiRuntimeEvent::Failed(message) => {
                self.set_activity(CommandActivity::Idle);
                self.append_transcript_event(TranscriptEvent::Error(message));
                self.panes.mark_stale();
            }
        }
    }

    fn apply_command_event(&mut self, event: &CommandEvent) {
        if let Some(progress) = &event.objective_progress {
            self.header.phase = progress.phase.as_str().to_string();
            self.header.active_task = progress.task_id.as_ref().map(ToString::to_string);
            self.panes.apply_objective_progress(progress);
        }
    }

    pub fn focus_next_pane(&mut self) {
        self.panes.next_pane();
        self.focus = TuiFocus::SidePane;
    }

    pub fn focus_previous_pane(&mut self) {
        self.panes.previous_pane();
        self.focus = TuiFocus::SidePane;
    }
}
