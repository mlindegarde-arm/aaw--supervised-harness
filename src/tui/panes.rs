use crate::runtime::{
    ObjectiveProgressEvent, ObjectiveProgressKind, PaneArtifactRow, PaneRunRow, PaneSection,
    PaneStateSnapshot, PaneTaskRow, PaneTicketRow,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardPaneSnapshot {
    pub phase2: PaneStateSnapshot,
    pub objective: Option<ObjectivePaneSnapshot>,
}

impl Default for DashboardPaneSnapshot {
    fn default() -> Self {
        Self {
            phase2: PaneStateSnapshot::default(),
            objective: None,
        }
    }
}

impl DashboardPaneSnapshot {
    pub fn with_phase2(phase2: PaneStateSnapshot) -> Self {
        Self {
            phase2,
            objective: None,
        }
    }

    pub fn apply_objective_progress(&mut self, event: &ObjectiveProgressEvent) {
        let objective = self
            .objective
            .get_or_insert_with(|| ObjectivePaneSnapshot::from_progress_event(event));
        objective.apply_progress_event(event);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectivePaneSnapshot {
    pub objective: ObjectiveHeader,
    pub acceptance: PaneSection<AcceptanceCriterionRow>,
    pub tasks: PaneSection<ObjectiveTaskRow>,
    pub workers: PaneSection<ObjectiveWorkerRow>,
    pub tickets: PaneSection<ObjectiveTicketRow>,
    pub validation: PaneSection<ObjectiveValidationRow>,
    pub remote_activity: PaneSection<PlannerExchangeRow>,
    pub transcript: PaneSection<ObjectiveTranscriptRow>,
}

impl ObjectivePaneSnapshot {
    fn from_progress_event(event: &ObjectiveProgressEvent) -> Self {
        Self {
            objective: ObjectiveHeader {
                id: event.objective_id.to_string(),
                phase: event.phase.as_str().to_string(),
                status: event.status.as_str().to_string(),
                message: safe_text(&event.message),
                updated_at: event.timestamp.clone(),
            },
            acceptance: PaneSection::Ready(Vec::new()),
            tasks: PaneSection::Ready(Vec::new()),
            workers: PaneSection::Ready(Vec::new()),
            tickets: PaneSection::Ready(Vec::new()),
            validation: PaneSection::Ready(Vec::new()),
            remote_activity: PaneSection::Ready(Vec::new()),
            transcript: PaneSection::Ready(Vec::new()),
        }
    }

    pub fn apply_progress_event(&mut self, event: &ObjectiveProgressEvent) {
        self.objective.phase = event.phase.as_str().to_string();
        self.objective.status = event.status.as_str().to_string();
        self.objective.message = safe_text(&event.message);
        self.objective.updated_at = event.timestamp.clone();
        push_limited(
            &mut self.transcript,
            ObjectiveTranscriptRow {
                timestamp: event.timestamp.clone(),
                phase: event.phase.as_str().to_string(),
                message: safe_text(&event.message),
            },
            8,
        );

        match event.kind {
            ObjectiveProgressKind::PlanningStarted => {
                upsert_remote_activity(
                    &mut self.remote_activity,
                    "planner",
                    "running",
                    &event.message,
                );
            }
            ObjectiveProgressKind::PlanningCompleted => {
                upsert_remote_activity(
                    &mut self.remote_activity,
                    "planner",
                    "accepted",
                    &event.message,
                );
            }
            ObjectiveProgressKind::PlanRejected => {
                upsert_remote_activity(
                    &mut self.remote_activity,
                    "planner",
                    "rejected",
                    &event.message,
                );
            }
            ObjectiveProgressKind::TaskCreated => {
                if let Some(task_id) = &event.task_id {
                    upsert_objective_task(
                        &mut self.tasks,
                        task_id.as_str(),
                        "queued",
                        &event.message,
                    );
                }
            }
            ObjectiveProgressKind::SupervisionStarted => {
                upsert_remote_activity(
                    &mut self.remote_activity,
                    "monitor",
                    "running",
                    &event.message,
                );
            }
            ObjectiveProgressKind::WorkerStarted => {
                if let Some(task_id) = &event.task_id {
                    upsert_objective_task(
                        &mut self.tasks,
                        task_id.as_str(),
                        "running",
                        &event.message,
                    );
                    upsert_worker(
                        &mut self.workers,
                        task_id.as_str(),
                        "running",
                        &event.message,
                    );
                }
            }
            ObjectiveProgressKind::WorkerCompleted => {
                if let Some(task_id) = &event.task_id {
                    upsert_objective_task(
                        &mut self.tasks,
                        task_id.as_str(),
                        "complete",
                        &event.message,
                    );
                    upsert_worker(
                        &mut self.workers,
                        task_id.as_str(),
                        "complete",
                        &event.message,
                    );
                }
            }
            ObjectiveProgressKind::TicketDetected => {
                if let Some(ticket_id) = &event.ticket_id {
                    upsert_ticket(
                        &mut self.tickets,
                        ticket_id.as_str(),
                        "open",
                        &event.message,
                    );
                }
                if let Some(task_id) = &event.task_id {
                    upsert_worker(&mut self.workers, task_id.as_str(), "stuck", &event.message);
                }
            }
            ObjectiveProgressKind::TicketResolutionStarted => {
                if let Some(ticket_id) = &event.ticket_id {
                    upsert_ticket(
                        &mut self.tickets,
                        ticket_id.as_str(),
                        "resolving",
                        &event.message,
                    );
                }
                upsert_remote_activity(
                    &mut self.remote_activity,
                    "resolver",
                    "running",
                    &event.message,
                );
            }
            ObjectiveProgressKind::TicketResolutionCompleted => {
                if let Some(ticket_id) = &event.ticket_id {
                    upsert_ticket(
                        &mut self.tickets,
                        ticket_id.as_str(),
                        "resolved",
                        &event.message,
                    );
                }
                upsert_remote_activity(
                    &mut self.remote_activity,
                    "resolver",
                    "accepted",
                    &event.message,
                );
            }
            ObjectiveProgressKind::WorkerResumed => {
                if let Some(task_id) = &event.task_id {
                    upsert_worker(
                        &mut self.workers,
                        task_id.as_str(),
                        "resuming",
                        &event.message,
                    );
                }
            }
            ObjectiveProgressKind::ValidationStarted => {
                upsert_validation(
                    &mut self.validation,
                    "acceptance",
                    "running",
                    &event.message,
                );
            }
            ObjectiveProgressKind::ValidationFailed => {
                upsert_validation(&mut self.validation, "acceptance", "failed", &event.message);
                upsert_acceptance(
                    &mut self.acceptance,
                    "acceptance",
                    "failing",
                    &event.message,
                );
            }
            ObjectiveProgressKind::RepairTaskCreated => {
                if let Some(task_id) = &event.task_id {
                    upsert_objective_task(
                        &mut self.tasks,
                        task_id.as_str(),
                        "queued",
                        &event.message,
                    );
                }
            }
            ObjectiveProgressKind::ValidationPassed => {
                upsert_validation(&mut self.validation, "acceptance", "passed", &event.message);
                upsert_acceptance(
                    &mut self.acceptance,
                    "acceptance",
                    "passing",
                    &event.message,
                );
            }
            ObjectiveProgressKind::Completed
            | ObjectiveProgressKind::Blocked
            | ObjectiveProgressKind::Failed
            | ObjectiveProgressKind::CancelRequested
            | ObjectiveProgressKind::Cancelled => {}
        }
    }

    pub fn row_count(&self) -> usize {
        2 + section_display_row_count(&self.tasks)
            + section_display_row_count(&self.workers)
            + section_display_row_count(&self.tickets)
            + section_display_row_count(&self.validation)
            + section_display_row_count(&self.remote_activity)
            + transcript_display_row_count(&self.transcript)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveHeader {
    pub id: String,
    pub phase: String,
    pub status: String,
    pub message: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceCriterionRow {
    pub id: String,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveTaskRow {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveWorkerRow {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveTicketRow {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveValidationRow {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerExchangeRow {
    pub id: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveTranscriptRow {
    pub timestamp: String,
    pub phase: String,
    pub message: String,
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
    snapshot: DashboardPaneSnapshot,
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
            snapshot: DashboardPaneSnapshot::default(),
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
        &self.snapshot.phase2
    }

    pub fn dashboard_snapshot(&self) -> &DashboardPaneSnapshot {
        &self.snapshot
    }

    pub fn objective(&self) -> Option<&ObjectivePaneSnapshot> {
        self.snapshot.objective.as_ref()
    }

    pub fn refresh(&self) -> PaneRefreshState {
        self.refresh
    }

    pub fn set_snapshot(&mut self, snapshot: PaneStateSnapshot) {
        self.snapshot.phase2 = sanitize_snapshot(snapshot);
        self.clamp_all();
        self.refresh.stale = false;
    }

    pub fn set_dashboard_snapshot(&mut self, snapshot: DashboardPaneSnapshot) {
        self.snapshot = sanitize_dashboard_snapshot(snapshot);
        self.clamp_all();
        self.refresh.stale = false;
    }

    pub fn apply_objective_progress(&mut self, event: &ObjectiveProgressEvent) {
        self.snapshot.apply_objective_progress(event);
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
        if let Some(objective) = &self.snapshot.objective {
            return objective.row_count();
        }
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
            PaneKind::Tasks => rows_from_tasks(&self.snapshot.phase2.tasks),
            PaneKind::Tickets => rows_from_tickets(&self.snapshot.phase2.tickets),
            PaneKind::Runs => rows_from_runs(&self.snapshot.phase2.runs),
            PaneKind::Artifacts => rows_from_artifacts(&self.snapshot.phase2.artifacts),
        }
    }

    pub fn pane_message(&self, pane: PaneKind) -> Option<String> {
        match pane {
            PaneKind::Tasks => section_message(&self.snapshot.phase2.tasks),
            PaneKind::Tickets => section_message(&self.snapshot.phase2.tickets),
            PaneKind::Runs => section_message(&self.snapshot.phase2.runs),
            PaneKind::Artifacts => section_message(&self.snapshot.phase2.artifacts),
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
            .clamp(rows_from_tasks(&self.snapshot.phase2.tasks).len());
        self.tickets
            .clamp(rows_from_tickets(&self.snapshot.phase2.tickets).len());
        self.runs
            .clamp(rows_from_runs(&self.snapshot.phase2.runs).len());
        self.artifacts
            .clamp(rows_from_artifacts(&self.snapshot.phase2.artifacts).len());
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

fn section_display_row_count<T>(section: &PaneSection<T>) -> usize {
    1 + match section {
        PaneSection::Ready(rows) if rows.is_empty() => 1,
        PaneSection::Ready(rows) => rows.len().min(4),
        PaneSection::Loading | PaneSection::Error(_) => 1,
    }
}

fn transcript_display_row_count(section: &PaneSection<ObjectiveTranscriptRow>) -> usize {
    1 + match section {
        PaneSection::Ready(rows) if rows.is_empty() => 1,
        PaneSection::Ready(rows) => rows.len().min(4),
        PaneSection::Loading | PaneSection::Error(_) => 0,
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

fn sanitize_dashboard_snapshot(snapshot: DashboardPaneSnapshot) -> DashboardPaneSnapshot {
    DashboardPaneSnapshot {
        phase2: sanitize_snapshot(snapshot.phase2),
        objective: snapshot.objective.map(sanitize_objective_snapshot),
    }
}

fn sanitize_objective_snapshot(mut snapshot: ObjectivePaneSnapshot) -> ObjectivePaneSnapshot {
    snapshot.objective.message = safe_text(&snapshot.objective.message);
    snapshot.acceptance = sanitize_section(snapshot.acceptance, |mut row| {
        row.description = safe_text(&row.description);
        row
    });
    snapshot.tasks = sanitize_section(snapshot.tasks, |mut row| {
        row.detail = safe_text(&row.detail);
        row
    });
    snapshot.workers = sanitize_section(snapshot.workers, |mut row| {
        row.detail = safe_text(&row.detail);
        row
    });
    snapshot.tickets = sanitize_section(snapshot.tickets, |mut row| {
        row.detail = safe_text(&row.detail);
        row
    });
    snapshot.validation = sanitize_section(snapshot.validation, |mut row| {
        row.detail = safe_text(&row.detail);
        row
    });
    snapshot.remote_activity = sanitize_section(snapshot.remote_activity, |mut row| {
        row.detail = safe_text(&row.detail);
        row
    });
    snapshot.transcript = sanitize_section(snapshot.transcript, |mut row| {
        row.message = safe_text(&row.message);
        row
    });
    snapshot
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

fn push_limited<T>(section: &mut PaneSection<T>, row: T, limit: usize) {
    let rows = ready_rows_mut(section);
    rows.insert(0, row);
    rows.truncate(limit);
}

fn ready_rows_mut<T>(section: &mut PaneSection<T>) -> &mut Vec<T> {
    if !matches!(section, PaneSection::Ready(_)) {
        *section = PaneSection::Ready(Vec::new());
    }
    match section {
        PaneSection::Ready(rows) => rows,
        PaneSection::Loading | PaneSection::Error(_) => unreachable!(),
    }
}

fn upsert_acceptance(
    section: &mut PaneSection<AcceptanceCriterionRow>,
    id: &str,
    status: &str,
    detail: &str,
) {
    upsert_row(section, id, status, detail, |id, status, detail| {
        AcceptanceCriterionRow {
            id,
            status,
            description: detail,
        }
    });
}

fn upsert_objective_task(
    section: &mut PaneSection<ObjectiveTaskRow>,
    id: &str,
    status: &str,
    detail: &str,
) {
    upsert_row(section, id, status, detail, |id, status, detail| {
        ObjectiveTaskRow { id, status, detail }
    });
}

fn upsert_worker(
    section: &mut PaneSection<ObjectiveWorkerRow>,
    id: &str,
    status: &str,
    detail: &str,
) {
    upsert_row(section, id, status, detail, |id, status, detail| {
        ObjectiveWorkerRow { id, status, detail }
    });
}

fn upsert_ticket(
    section: &mut PaneSection<ObjectiveTicketRow>,
    id: &str,
    status: &str,
    detail: &str,
) {
    upsert_row(section, id, status, detail, |id, status, detail| {
        ObjectiveTicketRow { id, status, detail }
    });
}

fn upsert_validation(
    section: &mut PaneSection<ObjectiveValidationRow>,
    id: &str,
    status: &str,
    detail: &str,
) {
    upsert_row(section, id, status, detail, |id, status, detail| {
        ObjectiveValidationRow { id, status, detail }
    });
}

fn upsert_remote_activity(
    section: &mut PaneSection<PlannerExchangeRow>,
    id: &str,
    status: &str,
    detail: &str,
) {
    upsert_row(section, id, status, detail, |id, status, detail| {
        PlannerExchangeRow { id, status, detail }
    });
}

fn upsert_row<T, F>(section: &mut PaneSection<T>, id: &str, status: &str, detail: &str, build: F)
where
    F: Fn(String, String, String) -> T,
    T: RowIdentity,
{
    let rows = ready_rows_mut(section);
    if let Some(row) = rows.iter_mut().find(|row| row.id() == id) {
        row.set_status(status);
        row.set_detail(detail);
        return;
    }
    rows.push(build(id.to_string(), status.to_string(), safe_text(detail)));
}

trait RowIdentity {
    fn id(&self) -> &str;
    fn set_status(&mut self, status: &str);
    fn set_detail(&mut self, detail: &str);
}

macro_rules! impl_row_identity {
    ($type:ty, $detail:ident) => {
        impl RowIdentity for $type {
            fn id(&self) -> &str {
                &self.id
            }

            fn set_status(&mut self, status: &str) {
                self.status = status.to_string();
            }

            fn set_detail(&mut self, detail: &str) {
                self.$detail = safe_text(detail);
            }
        }
    };
}

impl_row_identity!(AcceptanceCriterionRow, description);
impl_row_identity!(ObjectiveTaskRow, detail);
impl_row_identity!(ObjectiveWorkerRow, detail);
impl_row_identity!(ObjectiveTicketRow, detail);
impl_row_identity!(ObjectiveValidationRow, detail);
impl_row_identity!(PlannerExchangeRow, detail);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ArtifactId, ObjectiveId, ObjectiveStatus, RunId, RunStatus, TaskId, TaskStatus, TicketId,
        TicketStatus,
    };
    use crate::runtime::{ObjectiveProgressEvent, ObjectiveProgressKind, ObjectiveProgressPhase};

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ARTIFACT_ID: &str = "art_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OBJECTIVE_ID: &str = "objective_01ARZ3NDEKTSV4RRFFQ69G5FAV";

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

    #[test]
    fn objective_panel_reducer_tracks_lifecycle_rows() {
        let mut snapshot = DashboardPaneSnapshot::default();
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let ticket_id = TicketId::parse(TICKET_ID).unwrap();

        snapshot.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::PlanningStarted,
            ObjectiveProgressPhase::Planning,
            "planning objective",
            None,
            None,
        ));
        snapshot.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::WorkerStarted,
            ObjectiveProgressPhase::Running,
            "running local worker",
            Some(task_id.clone()),
            None,
        ));
        snapshot.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::TicketResolutionStarted,
            ObjectiveProgressPhase::Resolving,
            "remote resolver running",
            Some(task_id),
            Some(ticket_id),
        ));
        snapshot.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::ValidationPassed,
            ObjectiveProgressPhase::Complete,
            "validation passed",
            None,
            None,
        ));

        let objective = snapshot.objective.as_ref().unwrap();
        assert_eq!(objective.objective.phase, "complete");
        assert!(
            matches!(&objective.workers, PaneSection::Ready(rows) if rows[0].status == "running")
        );
        assert!(
            matches!(&objective.tickets, PaneSection::Ready(rows) if rows[0].status == "resolving")
        );
        assert!(
            matches!(&objective.validation, PaneSection::Ready(rows) if rows[0].status == "passed")
        );
        assert!(
            matches!(&objective.remote_activity, PaneSection::Ready(rows) if rows.iter().any(|row| row.id == "resolver" && row.status == "running"))
        );
    }

    #[test]
    fn objective_panel_rows_make_side_pane_scrollable_without_phase2_rows() {
        let mut panes = SidePaneState::default();
        panes.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::PlanningStarted,
            ObjectiveProgressPhase::Planning,
            "planning objective",
            None,
            None,
        ));

        assert!(panes.rows_for(PaneKind::Tasks).is_empty());
        assert!(panes.active_row_count() > 0);
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

    fn objective_event(
        kind: ObjectiveProgressKind,
        phase: ObjectiveProgressPhase,
        message: &str,
        task_id: Option<TaskId>,
        ticket_id: Option<TicketId>,
    ) -> ObjectiveProgressEvent {
        let mut event = ObjectiveProgressEvent::new(
            kind,
            ObjectiveId::parse(OBJECTIVE_ID).unwrap(),
            phase,
            match phase {
                ObjectiveProgressPhase::Planning => ObjectiveStatus::Planning,
                ObjectiveProgressPhase::Ready => ObjectiveStatus::Ready,
                ObjectiveProgressPhase::Running
                | ObjectiveProgressPhase::Resolving
                | ObjectiveProgressPhase::Validating => ObjectiveStatus::Running,
                ObjectiveProgressPhase::Blocked => ObjectiveStatus::Blocked,
                ObjectiveProgressPhase::Complete => ObjectiveStatus::Complete,
                ObjectiveProgressPhase::Failed => ObjectiveStatus::Failed,
                ObjectiveProgressPhase::Cancelled => ObjectiveStatus::Cancelled,
            },
            message,
            "2026-05-14T00:00:00Z",
        );
        event.task_id = task_id;
        event.ticket_id = ticket_id;
        event
    }
}
