use crate::domain::{RunStatus, TaskStatus, TicketStatus};
use crate::runtime::{CommandEventLevel, SuperviseProgressPhase};
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiTheme {
    pub base: Style,
    pub muted: Style,
    pub header: Style,
    pub title: Style,
    pub selected: Style,
    pub focused_border: Style,
    pub border: Style,
    pub success: Style,
    pub warning: Style,
    pub error: Style,
    pub stdout: Style,
    pub stderr: Style,
}

impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            base: Style::default().fg(Color::Reset).bg(Color::Reset),
            muted: Style::default().fg(Color::Gray),
            header: Style::default().add_modifier(Modifier::BOLD),
            title: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            selected: Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            focused_border: Style::default().fg(Color::Yellow),
            border: Style::default().fg(Color::DarkGray),
            success: Style::default().fg(Color::Green),
            warning: Style::default().fg(Color::Yellow),
            error: Style::default().fg(Color::Red),
            stdout: Style::default().fg(Color::Reset),
            stderr: Style::default().fg(Color::LightRed),
        }
    }
}

impl TuiTheme {
    pub fn task_status(&self, status: TaskStatus) -> Style {
        match status {
            TaskStatus::Complete => self.success,
            TaskStatus::Stuck | TaskStatus::Failed => self.error,
            TaskStatus::Running => self.warning,
            TaskStatus::Ready => self.base,
        }
    }

    pub fn ticket_status(&self, status: TicketStatus) -> Style {
        match status {
            TicketStatus::Open | TicketStatus::Failed => self.error,
            TicketStatus::Resolving => self.warning,
            TicketStatus::Resolved => self.success,
        }
    }

    pub fn run_status(&self, status: RunStatus) -> Style {
        match status {
            RunStatus::Complete => self.success,
            RunStatus::Stuck | RunStatus::Failed => self.error,
            RunStatus::Running => self.warning,
        }
    }

    pub fn event_level(&self, level: CommandEventLevel) -> Style {
        match level {
            CommandEventLevel::Info => self.base,
            CommandEventLevel::Warn => self.warning,
            CommandEventLevel::Error => self.error,
        }
    }

    pub fn progress_phase(&self, phase: SuperviseProgressPhase) -> Style {
        match phase {
            SuperviseProgressPhase::Complete => self.success,
            SuperviseProgressPhase::Failed
            | SuperviseProgressPhase::Stuck
            | SuperviseProgressPhase::Cancelled => self.error,
            SuperviseProgressPhase::Cancelling => self.warning,
            SuperviseProgressPhase::InspectTask
            | SuperviseProgressPhase::RunTask
            | SuperviseProgressPhase::ResolveTicket
            | SuperviseProgressPhase::ResumeTask => self.base,
        }
    }
}
