use crate::runtime::{
    PaneArtifactRow, PaneRunRow, PaneSection, PaneStateSnapshot, PaneTaskRow, PaneTicketRow,
};
use crate::tui::transcript::sanitize_untrusted_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Tasks,
    Tickets,
    Runs,
    Artifacts,
}

impl PaneKind {
    pub const ALL: [Self; 4] = [Self::Tasks, Self::Tickets, Self::Runs, Self::Artifacts];

    pub fn title(self) -> &'static str {
        match self {
            Self::Tasks => "Tasks",
            Self::Tickets => "Tickets",
            Self::Runs => "Runs",
            Self::Artifacts => "Artifacts",
        }
    }

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|pane| *pane == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let idx = Self::ALL.iter().position(|pane| *pane == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRowView {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaneListState {
    pub selected: usize,
    pub scroll: u16,
}

impl PaneListState {
    pub fn clamp(&mut self, row_count: usize) {
        if row_count == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(row_count - 1);
    }

    pub fn select_next(&mut self, row_count: usize) {
        if row_count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1).min(row_count - 1);
    }

    pub fn select_previous(&mut self, row_count: usize) {
        if row_count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll = self.scroll.saturating_add(lines);
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.scroll = self.scroll.saturating_sub(lines);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaneRefreshState {
    pub generation: u64,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidePaneState {
    snapshot: PaneStateSnapshot,
    active: PaneKind,
    tasks: PaneListState,
    tickets: PaneListState,
    runs: PaneListState,
    artifacts: PaneListState,
    refresh: PaneRefreshState,
}

impl Default for SidePaneState {
    fn default() -> Self {
        Self {
            snapshot: PaneStateSnapshot::default(),
            active: PaneKind::Tasks,
            tasks: PaneListState::default(),
            tickets: PaneListState::default(),
            runs: PaneListState::default(),
            artifacts: PaneListState::default(),
            refresh: PaneRefreshState::default(),
        }
    }
}

impl SidePaneState {
    pub fn active(&self) -> PaneKind {
        self.active
    }

    pub fn snapshot(&self) -> &PaneStateSnapshot {
        &self.snapshot
    }

    pub fn refresh(&self) -> PaneRefreshState {
        self.refresh
    }

    pub fn set_snapshot(&mut self, snapshot: PaneStateSnapshot) {
        self.snapshot = sanitize_snapshot(snapshot);
        self.clamp_all();
        self.refresh.stale = false;
    }

    pub fn mark_stale(&mut self) {
        self.refresh.generation = self.refresh.generation.saturating_add(1);
        self.refresh.stale = true;
    }

    pub fn set_active(&mut self, pane: PaneKind) {
        self.active = pane;
        let row_count = self.active_row_count();
        self.active_state_mut().clamp(row_count);
    }

    pub fn next_pane(&mut self) {
        self.set_active(self.active.next());
    }

    pub fn previous_pane(&mut self) {
        self.set_active(self.active.previous());
    }

    pub fn active_state(&self) -> PaneListState {
        match self.active {
            PaneKind::Tasks => self.tasks,
            PaneKind::Tickets => self.tickets,
            PaneKind::Runs => self.runs,
            PaneKind::Artifacts => self.artifacts,
        }
    }

    pub fn active_row_count(&self) -> usize {
        self.rows_for(self.active).len()
    }

    pub fn select_next(&mut self) {
        let row_count = self.active_row_count();
        self.active_state_mut().select_next(row_count);
    }

    pub fn select_previous(&mut self) {
        let row_count = self.active_row_count();
        self.active_state_mut().select_previous(row_count);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.active_state_mut().scroll_down(lines);
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.active_state_mut().scroll_up(lines);
    }

    pub fn rows_for(&self, pane: PaneKind) -> Vec<PaneRowView> {
        match pane {
            PaneKind::Tasks => rows_from_tasks(&self.snapshot.tasks),
            PaneKind::Tickets => rows_from_tickets(&self.snapshot.tickets),
            PaneKind::Runs => rows_from_runs(&self.snapshot.runs),
            PaneKind::Artifacts => rows_from_artifacts(&self.snapshot.artifacts),
        }
    }

    pub fn pane_message(&self, pane: PaneKind) -> Option<String> {
        match pane {
            PaneKind::Tasks => section_message(&self.snapshot.tasks),
            PaneKind::Tickets => section_message(&self.snapshot.tickets),
            PaneKind::Runs => section_message(&self.snapshot.runs),
            PaneKind::Artifacts => section_message(&self.snapshot.artifacts),
        }
    }

    pub fn selected_row(&self) -> Option<PaneRowView> {
        let rows = self.rows_for(self.active);
        rows.get(self.active_state().selected).cloned()
    }

    fn active_state_mut(&mut self) -> &mut PaneListState {
        match self.active {
            PaneKind::Tasks => &mut self.tasks,
            PaneKind::Tickets => &mut self.tickets,
            PaneKind::Runs => &mut self.runs,
            PaneKind::Artifacts => &mut self.artifacts,
        }
    }

    fn clamp_all(&mut self) {
        self.tasks
            .clamp(rows_from_tasks(&self.snapshot.tasks).len());
        self.tickets
            .clamp(rows_from_tickets(&self.snapshot.tickets).len());
        self.runs.clamp(rows_from_runs(&self.snapshot.runs).len());
        self.artifacts
            .clamp(rows_from_artifacts(&self.snapshot.artifacts).len());
    }
}

fn rows_from_tasks(section: &PaneSection<PaneTaskRow>) -> Vec<PaneRowView> {
    match section {
        PaneSection::Ready(rows) => rows
            .iter()
            .map(|row| PaneRowView {
                id: row.task_id.to_string(),
                status: row.status.as_str().to_string(),
                detail: format!(
                    "{} | latest {} | updated {}",
                    row.title,
                    row.latest_run_id
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".to_string()),
                    row.updated_at
                ),
            })
            .collect(),
        PaneSection::Loading | PaneSection::Error(_) => Vec::new(),
    }
}

fn rows_from_tickets(section: &PaneSection<PaneTicketRow>) -> Vec<PaneRowView> {
    match section {
        PaneSection::Ready(rows) => rows
            .iter()
            .map(|row| PaneRowView {
                id: row.ticket_id.to_string(),
                status: row.status.as_str().to_string(),
                detail: format!(
                    "{} | {} | {} | {}",
                    row.task_id, row.run_id, row.blocked_on, row.question
                ),
            })
            .collect(),
        PaneSection::Loading | PaneSection::Error(_) => Vec::new(),
    }
}

fn rows_from_runs(section: &PaneSection<PaneRunRow>) -> Vec<PaneRowView> {
    match section {
        PaneSection::Ready(rows) => rows
            .iter()
            .map(|row| PaneRowView {
                id: row.run_id.to_string(),
                status: row.status.as_str().to_string(),
                detail: format!(
                    "{} | cycle {} | {} | {}",
                    row.task_id,
                    row.escalation_cycle,
                    row.current_phase.as_deref().unwrap_or("-"),
                    row.latest_artifact_path.as_deref().unwrap_or("-")
                ),
            })
            .collect(),
        PaneSection::Loading | PaneSection::Error(_) => Vec::new(),
    }
}

fn rows_from_artifacts(section: &PaneSection<PaneArtifactRow>) -> Vec<PaneRowView> {
    match section {
        PaneSection::Ready(rows) => rows
            .iter()
            .map(|row| PaneRowView {
                id: row.artifact_id.to_string(),
                status: row.kind.clone(),
                detail: format!(
                    "{} | {} bytes | {} | task {}",
                    row.path, row.byte_len, row.sha256_prefix, row.task_id
                ),
            })
            .collect(),
        PaneSection::Loading | PaneSection::Error(_) => Vec::new(),
    }
}

fn section_message<T>(section: &PaneSection<T>) -> Option<String> {
    match section {
        PaneSection::Loading => Some("loading...".to_string()),
        PaneSection::Ready(rows) if rows.is_empty() => Some("no rows".to_string()),
        PaneSection::Ready(_) => None,
        PaneSection::Error(message) => Some(format!("error: {message}")),
    }
}

fn sanitize_snapshot(snapshot: PaneStateSnapshot) -> PaneStateSnapshot {
    PaneStateSnapshot {
        tasks: sanitize_section(snapshot.tasks, sanitize_task_row),
        tickets: sanitize_section(snapshot.tickets, sanitize_ticket_row),
        runs: sanitize_section(snapshot.runs, sanitize_run_row),
        artifacts: sanitize_section(snapshot.artifacts, sanitize_artifact_row),
    }
}

fn sanitize_section<T, F>(section: PaneSection<T>, sanitize_row: F) -> PaneSection<T>
where
    F: Fn(T) -> T,
{
    match section {
        PaneSection::Loading => PaneSection::Loading,
        PaneSection::Ready(rows) => {
            PaneSection::Ready(rows.into_iter().map(sanitize_row).collect())
        }
        PaneSection::Error(message) => PaneSection::Error(safe_text(&message)),
    }
}

fn sanitize_task_row(mut row: PaneTaskRow) -> PaneTaskRow {
    row.title = safe_text(&row.title);
    row.updated_at = safe_text(&row.updated_at);
    row
}

fn sanitize_ticket_row(mut row: PaneTicketRow) -> PaneTicketRow {
    row.blocked_on = safe_text(&row.blocked_on);
    row.question = safe_text(&row.question);
    row
}

fn sanitize_run_row(mut row: PaneRunRow) -> PaneRunRow {
    row.current_phase = row.current_phase.map(|text| safe_text(&text));
    row.latest_artifact_path = row.latest_artifact_path.map(|text| safe_text(&text));
    row
}

fn sanitize_artifact_row(mut row: PaneArtifactRow) -> PaneArtifactRow {
    row.kind = safe_text(&row.kind);
    row.path = safe_text(&row.path);
    row.sha256_prefix = safe_text(&row.sha256_prefix);
    row
}

fn safe_text(text: &str) -> String {
    sanitize_untrusted_text(text).text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ArtifactId, RunId, RunStatus, TaskId, TaskStatus, TicketStatus};

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ARTIFACT_ID: &str = "art_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn panes_switch_and_preserve_selection_per_pane() {
        let mut panes = SidePaneState::default();
        panes.set_snapshot(snapshot_with_rows());

        panes.select_next();
        assert_eq!(panes.active_state().selected, 1);

        panes.next_pane();
        assert_eq!(panes.active(), PaneKind::Tickets);
        assert_eq!(panes.active_state().selected, 0);

        panes.previous_pane();
        assert_eq!(panes.active(), PaneKind::Tasks);
        assert_eq!(panes.active_state().selected, 1);
    }

    #[test]
    fn panes_expose_loading_empty_and_error_messages() {
        let mut panes = SidePaneState::default();
        assert_eq!(
            panes.pane_message(PaneKind::Tasks).as_deref(),
            Some("loading...")
        );

        panes.set_snapshot(PaneStateSnapshot {
            tasks: PaneSection::Ready(Vec::new()),
            tickets: PaneSection::Error("ticket store unavailable".to_string()),
            runs: PaneSection::Loading,
            artifacts: PaneSection::Ready(Vec::new()),
        });

        assert_eq!(
            panes.pane_message(PaneKind::Tasks).as_deref(),
            Some("no rows")
        );
        assert_eq!(
            panes.pane_message(PaneKind::Tickets).as_deref(),
            Some("error: ticket store unavailable")
        );
    }

    #[test]
    fn panes_sanitize_untrusted_row_text() {
        let mut snapshot = snapshot_with_rows();
        if let PaneSection::Ready(rows) = &mut snapshot.tasks {
            rows[0].title = "fix\x1b[2J OPENAI_API_KEY=sk-test-secret".to_string();
        }

        let mut panes = SidePaneState::default();
        panes.set_snapshot(snapshot);

        let row = panes.rows_for(PaneKind::Tasks).remove(0);
        assert!(!row.detail.contains('\x1b'));
        assert!(!row.detail.contains("sk-test-secret"));
        assert!(row.detail.contains("[REDACTED"));
    }

    #[test]
    fn panes_track_refresh_staleness() {
        let mut panes = SidePaneState::default();
        panes.mark_stale();
        assert!(panes.refresh().stale);
        assert_eq!(panes.refresh().generation, 1);

        panes.set_snapshot(snapshot_with_rows());
        assert!(!panes.refresh().stale);
        assert_eq!(panes.refresh().generation, 1);
    }

    fn snapshot_with_rows() -> PaneStateSnapshot {
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let run_id = RunId::parse(RUN_ID).unwrap();
        PaneStateSnapshot {
            tasks: PaneSection::Ready(vec![
                PaneTaskRow {
                    task_id: task_id.clone(),
                    status: TaskStatus::Ready,
                    title: "Fix parser".to_string(),
                    latest_run_id: None,
                    updated_at: "2026-05-14T00:00:00Z".to_string(),
                },
                PaneTaskRow {
                    task_id: task_id.clone(),
                    status: TaskStatus::Running,
                    title: "Run tests".to_string(),
                    latest_run_id: Some(run_id.clone()),
                    updated_at: "2026-05-14T00:01:00Z".to_string(),
                },
            ]),
            tickets: PaneSection::Ready(vec![PaneTicketRow {
                ticket_id: crate::domain::TicketId::parse(TICKET_ID).unwrap(),
                status: TicketStatus::Open,
                task_id: task_id.clone(),
                run_id: run_id.clone(),
                blocked_on: "ambiguous error".to_string(),
                question: "What should happen?".to_string(),
            }]),
            runs: PaneSection::Ready(vec![PaneRunRow {
                run_id,
                task_id: task_id.clone(),
                status: RunStatus::Running,
                escalation_cycle: 1,
                current_phase: Some("validation".to_string()),
                latest_artifact_path: Some("logs/run.txt".to_string()),
            }]),
            artifacts: PaneSection::Ready(vec![PaneArtifactRow {
                artifact_id: ArtifactId::parse(ARTIFACT_ID).unwrap(),
                kind: "log".to_string(),
                path: "logs/run.txt".to_string(),
                byte_len: 128,
                sha256_prefix: "abcd1234".to_string(),
                task_id,
                run_id: None,
                ticket_id: None,
            }]),
        }
    }
}
