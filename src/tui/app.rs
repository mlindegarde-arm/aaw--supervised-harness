use crate::completion::{
    CompletionContext, CompletionEngine, CompletionStateView, ObjectiveCompletionItem,
    ObjectiveCompletionScope, TaskCompletionItem, TaskCompletionScope, TicketCompletionItem,
    TicketCompletionScope,
};
use crate::domain::{ObjectiveStatus, TaskId, TicketId};
use crate::error::{HarnessError, HarnessResult};
use crate::runtime::{
    CommandCatalog, CommandExit, CommandStatus, ObjectiveProgressEvent, ObjectiveProgressKind,
    ObjectiveProgressPhase, PaneRunRow, PaneSection, PaneStateSnapshot, TuiRuntimeEvent, build_cli,
};
use crate::state::Objective;
use crate::tui::app_state::{CommandActivity, TuiAppState};
use crate::tui::composer::{ComposerOutcome, ComposerState, InputMode};
use crate::tui::input::{ComposerCommand, KeyCode, KeyEvent, KeyModifiers, apply_command};
use crate::tui::panes::DashboardPaneSnapshot;
use crate::tui::render::render_app;
use crate::tui::runtime_bridge::{RuntimeBridge, RuntimeServiceFactory};
use crate::tui::shell_escape::{
    DefaultShellEscapeRunner, ShellEscapeRunner, default_shell_escape_runner, run_shell_escape,
};
use crossterm::event::{self, Event, KeyCode as CrosstermKeyCode, KeyModifiers as CrosstermMods};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub trait TerminalMode {
    fn enter(&mut self) -> HarnessResult<()>;
    fn leave(&mut self) -> HarnessResult<()>;
}

pub struct TerminalCleanupGuard<M>
where
    M: TerminalMode,
{
    mode: M,
    active: bool,
}

impl<M> TerminalCleanupGuard<M>
where
    M: TerminalMode,
{
    pub fn enter(mut mode: M) -> HarnessResult<Self> {
        mode.enter()?;
        Ok(Self { mode, active: true })
    }

    pub fn leave(&mut self) -> HarnessResult<()> {
        if self.active {
            self.mode.leave()?;
            self.active = false;
        }
        Ok(())
    }
}

impl<M> Drop for TerminalCleanupGuard<M>
where
    M: TerminalMode,
{
    fn drop(&mut self) {
        if self.active {
            let _ = self.mode.leave();
            self.active = false;
        }
    }
}

pub struct CrosstermTerminalMode;

impl TerminalMode for CrosstermTerminalMode {
    fn enter(&mut self) -> HarnessResult<()> {
        enable_raw_mode().map_err(io_error("enable terminal raw mode"))?;
        execute!(io::stdout(), EnterAlternateScreen).map_err(io_error("enter alternate screen"))?;
        Ok(())
    }

    fn leave(&mut self) -> HarnessResult<()> {
        let raw = disable_raw_mode().map_err(io_error("disable terminal raw mode"));
        let screen = execute!(io::stdout(), LeaveAlternateScreen)
            .map_err(io_error("leave alternate screen"));
        raw.and(screen)
    }
}

pub struct TuiApp<R>
where
    R: ShellEscapeRunner + Clone + Send + 'static,
{
    pub state: TuiAppState,
    composer: ComposerState,
    runtime: RuntimeBridge,
    completion_engine: CompletionEngine,
    completion_state: ServiceCompletionState,
    catalog: CommandCatalog,
    repo: Option<PathBuf>,
    shell_runner: R,
    exit_requested: bool,
}

impl TuiApp<DefaultShellEscapeRunner> {
    pub fn new(service_factory: RuntimeServiceFactory) -> HarnessResult<Self> {
        Ok(Self::with_shell_runner(
            service_factory,
            default_shell_escape_runner()?,
        ))
    }
}

impl<R> TuiApp<R>
where
    R: ShellEscapeRunner + Clone + Send + 'static,
{
    pub fn with_shell_runner(service_factory: RuntimeServiceFactory, shell_runner: R) -> Self {
        let completion_state = ServiceCompletionState::new(service_factory.clone());
        let mut app = Self {
            state: TuiAppState::default(),
            composer: ComposerState::new(),
            runtime: RuntimeBridge::new(service_factory),
            completion_engine: CompletionEngine::new(),
            completion_state,
            catalog: build_cli(),
            repo: std::env::current_dir().ok(),
            shell_runner,
            exit_requested: false,
        };
        app.refresh_panes();
        app.refresh_completion();
        app.sync_composer_view();
        app
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let command = crate::tui::input::command_for_key(key);
        self.handle_command(command);
    }

    pub fn handle_command(&mut self, command: ComposerCommand) {
        if self.runtime.is_running() {
            if command == ComposerCommand::Interrupt && self.runtime.cancel() {
                self.state.set_activity(CommandActivity::Cancelling);
            }
            return;
        }

        if self.handle_app_command(command) {
            self.sync_composer_view();
            return;
        }

        match command {
            ComposerCommand::Complete if self.composer.input_mode() == InputMode::PlainPrompt => {
                self.state.append_runtime_event(TuiRuntimeEvent::Stderr(
                    "prompt text will start an objective; use / for harness commands".to_string(),
                ));
                self.sync_composer_view();
                return;
            }
            ComposerCommand::Complete if !self.composer.suggestions_visible() => {
                self.refresh_completion();
            }
            ComposerCommand::SubmitOrApplySuggestion
            | ComposerCommand::SelectPreviousOrHistoryPrevious
            | ComposerCommand::SelectNextOrHistoryNext
                if !self.composer.suggestions_visible() =>
            {
                self.refresh_completion();
            }
            _ => {}
        }
        let outcome = apply_command(&mut self.composer, command);
        let refresh_after_outcome = matches!(outcome, ComposerOutcome::AppliedSuggestion);
        self.handle_composer_outcome(outcome);
        if refresh_after_outcome
            || matches!(
                command,
                ComposerCommand::Insert(_)
                    | ComposerCommand::Backspace
                    | ComposerCommand::MoveLeft
                    | ComposerCommand::MoveRight
                    | ComposerCommand::MoveToStart
                    | ComposerCommand::MoveToEnd
                    | ComposerCommand::ClearBeforeCursor
                    | ComposerCommand::DeletePreviousWord
            )
        {
            self.refresh_completion();
        }
        self.sync_composer_view();
    }

    pub fn drain_runtime_events(&mut self) {
        for event in self.runtime.drain() {
            self.apply_runtime_event(event);
        }
        if self.state.panes.refresh().stale {
            self.refresh_panes();
        }
        self.sync_composer_view();
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    fn handle_composer_outcome(&mut self, outcome: ComposerOutcome) {
        match outcome {
            ComposerOutcome::Submitted(command) => self.submit(command),
            ComposerOutcome::Blocked { hint } => {
                self.state
                    .append_runtime_event(TuiRuntimeEvent::Failed(hint));
            }
            ComposerOutcome::ExitRequested => {
                self.exit_requested = true;
            }
            ComposerOutcome::Edited
            | ComposerOutcome::AppliedSuggestion
            | ComposerOutcome::Cleared
            | ComposerOutcome::Noop => {}
        }
    }

    fn submit(&mut self, command: String) {
        if let Some(shell_command) = command.strip_prefix('!') {
            let runner = self.shell_runner.clone();
            let shell_command = shell_command.to_string();
            match self
                .runtime
                .start_event_worker(move |sender, cancellation| {
                    if cancellation.load(Ordering::SeqCst) {
                        let _ =
                            sender.send(TuiRuntimeEvent::CancelAcknowledged { next_command: None });
                        return;
                    }
                    for event in run_shell_escape(&shell_command, &runner, cancellation.as_ref()) {
                        let terminal = matches!(
                            event,
                            TuiRuntimeEvent::CommandFinished(_)
                                | TuiRuntimeEvent::CancelAcknowledged { .. }
                                | TuiRuntimeEvent::Failed(_)
                        );
                        let _ = sender.send(event);
                        if terminal {
                            break;
                        }
                    }
                }) {
                Ok(()) => self.state.set_activity(CommandActivity::Running),
                Err(err) => self
                    .state
                    .append_runtime_event(TuiRuntimeEvent::Failed(err.to_string())),
            }
            return;
        }

        let routed_command = route_prompt_first_command(&command, self.repo.as_deref());
        if routed_command != command {
            self.state
                .append_runtime_event(TuiRuntimeEvent::Stderr(format!(
                    "starting objective: {routed_command}"
                )));
        }
        match self.runtime.start_command(routed_command) {
            Ok(()) => self.state.set_activity(CommandActivity::Running),
            Err(err) => self
                .state
                .append_runtime_event(TuiRuntimeEvent::Failed(err.to_string())),
        }
    }

    fn apply_runtime_event(&mut self, event: TuiRuntimeEvent) {
        self.state.append_runtime_event(event);
    }

    fn handle_app_command(&mut self, command: ComposerCommand) -> bool {
        match command {
            ComposerCommand::NextPane => {
                self.state.focus_next_pane();
                true
            }
            ComposerCommand::PreviousPane => {
                self.state.focus_previous_pane();
                true
            }
            ComposerCommand::ScrollPageUp => {
                if self.state.focus == crate::tui::app_state::TuiFocus::SidePane
                    && self.state.panes.active_row_count() > 0
                {
                    self.state.panes.scroll_up(8);
                } else {
                    self.state.transcript.scroll_up(8);
                    self.state.focus = crate::tui::app_state::TuiFocus::Transcript;
                }
                true
            }
            ComposerCommand::ScrollPageDown => {
                if self.state.focus == crate::tui::app_state::TuiFocus::SidePane
                    && self.state.panes.active_row_count() > 0
                {
                    self.state.panes.scroll_down(8);
                } else {
                    self.state.transcript.scroll_down(8);
                    self.state.focus = crate::tui::app_state::TuiFocus::Transcript;
                }
                true
            }
            ComposerCommand::Escape
                if self.state.focus != crate::tui::app_state::TuiFocus::Composer =>
            {
                self.state.focus = crate::tui::app_state::TuiFocus::Composer;
                true
            }
            _ => false,
        }
    }

    fn refresh_completion(&mut self) {
        let context = CompletionContext {
            state: &self.completion_state,
            repo: self.repo.clone(),
            catalog: &self.catalog,
        };
        if let Err(err) = self
            .composer
            .refresh_completion(&self.completion_engine, &context)
        {
            self.state
                .append_runtime_event(TuiRuntimeEvent::Failed(err.to_string()));
        }
    }

    fn refresh_panes(&mut self) {
        let service = self.completion_state.service();
        let tasks = match service.list_tasks() {
            Ok(tasks) => PaneSection::Ready(
                tasks
                    .into_iter()
                    .map(|task| crate::runtime::PaneTaskRow {
                        task_id: task.id,
                        status: task.status,
                        title: task.title,
                        latest_run_id: None,
                        updated_at: task.updated_at,
                    })
                    .collect(),
            ),
            Err(err) => PaneSection::Error(err.to_string()),
        };
        let tickets = match service.list_tickets() {
            Ok(tickets) => PaneSection::Ready(
                tickets
                    .into_iter()
                    .map(|ticket| crate::runtime::PaneTicketRow {
                        ticket_id: ticket.id,
                        status: ticket.status,
                        task_id: ticket.task_id,
                        run_id: ticket.run_id,
                        blocked_on: ticket.blocked_on,
                        question: ticket.question,
                    })
                    .collect(),
            ),
            Err(err) => PaneSection::Error(err.to_string()),
        };
        let phase2 = PaneStateSnapshot {
            tasks,
            tickets,
            runs: PaneSection::Ready(Vec::<PaneRunRow>::new()),
            artifacts: PaneSection::Ready(Vec::new()),
        };
        let mut dashboard = DashboardPaneSnapshot::with_phase2(phase2);
        if let Ok(objectives) = service.list_objectives(None)
            && let Some(objective) = select_dashboard_objective(&objectives)
        {
            hydrate_objective_dashboard(service.as_ref(), &mut dashboard, objective);
        }
        self.state.set_dashboard_snapshot(dashboard);
    }

    fn sync_composer_view(&mut self) {
        self.state.composer.text = self.composer.text().to_string();
        self.state.composer.cursor = self.composer.cursor();
        if self.state.composer.disabled {
            self.state.composer.suggestions.clear();
            self.state.composer.hint = None;
        } else {
            self.state.composer.suggestions = self.composer.suggestion_rows();
            self.state.composer.hint = self.composer.hint_text();
        }
    }
}

fn route_prompt_first_command(input: &str, repo: Option<&std::path::Path>) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with('/') || trimmed.starts_with('!') || legacy_command_like(trimmed) {
        return trimmed.to_string();
    }
    let escaped_prompt = shell_quote(trimmed);
    let mut command = String::from("objective start --supervise --prompt ");
    command.push_str(&escaped_prompt);
    if let Some(repo) = repo.and_then(|path| path.to_str()) {
        command.push_str(" --repo ");
        command.push_str(&shell_quote(repo));
    }
    command
}

fn select_dashboard_objective(objectives: &[Objective]) -> Option<&Objective> {
    objectives
        .iter()
        .rev()
        .find(|objective| {
            matches!(
                objective.status,
                ObjectiveStatus::Planning
                    | ObjectiveStatus::Ready
                    | ObjectiveStatus::Running
                    | ObjectiveStatus::Blocked
            )
        })
        .or_else(|| objectives.last())
}

fn hydrate_objective_dashboard(
    service: &dyn crate::service::HarnessService,
    dashboard: &mut DashboardPaneSnapshot,
    objective: &Objective,
) {
    let mut applied = false;
    if let Ok(plan) = service.get_objective_plan(&objective.id)
        && let Some(events) = plan.data.get("events").and_then(|value| value.as_array())
    {
        for event in events {
            if let Some(progress) = progress_from_persisted_event(objective, event) {
                dashboard.apply_objective_progress(&progress);
                applied = true;
            }
        }
    }

    if !applied {
        dashboard.apply_objective_progress(&progress_from_objective(objective));
    }
}

fn progress_from_objective(objective: &Objective) -> ObjectiveProgressEvent {
    let (kind, phase) = match objective.status {
        ObjectiveStatus::Planning => (
            ObjectiveProgressKind::PlanningStarted,
            ObjectiveProgressPhase::Planning,
        ),
        ObjectiveStatus::Ready => (
            ObjectiveProgressKind::PlanningCompleted,
            ObjectiveProgressPhase::Ready,
        ),
        ObjectiveStatus::Running => (
            ObjectiveProgressKind::SupervisionStarted,
            ObjectiveProgressPhase::Running,
        ),
        ObjectiveStatus::Blocked => (
            ObjectiveProgressKind::Blocked,
            ObjectiveProgressPhase::Blocked,
        ),
        ObjectiveStatus::Complete => (
            ObjectiveProgressKind::Completed,
            ObjectiveProgressPhase::Complete,
        ),
        ObjectiveStatus::Failed => (
            ObjectiveProgressKind::Failed,
            ObjectiveProgressPhase::Failed,
        ),
        ObjectiveStatus::Cancelled => (
            ObjectiveProgressKind::Cancelled,
            ObjectiveProgressPhase::Cancelled,
        ),
    };
    ObjectiveProgressEvent::new(
        kind,
        objective.id.clone(),
        phase,
        objective.status,
        objective.summary.clone(),
        objective.updated_at.clone(),
    )
}

fn progress_from_persisted_event(
    objective: &Objective,
    event: &serde_json::Value,
) -> Option<ObjectiveProgressEvent> {
    let event_type = event.get("event_type")?.as_str()?;
    let message = event
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or(event_type);
    let timestamp = event
        .get("created_at")
        .and_then(|value| value.as_str())
        .unwrap_or(&objective.updated_at);
    let payload = event
        .get("payload_json")
        .and_then(|value| value.as_str())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let (kind, phase, status) = objective_event_type_state(event_type, objective.status)?;
    let mut progress = ObjectiveProgressEvent::new(
        kind,
        objective.id.clone(),
        phase,
        status,
        message.to_string(),
        timestamp.to_string(),
    );
    progress.task_id = payload
        .get("task_id")
        .and_then(|value| value.as_str())
        .and_then(|value| TaskId::parse(value).ok());
    progress.ticket_id = payload
        .get("ticket_id")
        .and_then(|value| value.as_str())
        .and_then(|value| TicketId::parse(value).ok());
    Some(progress)
}

fn objective_event_type_state(
    event_type: &str,
    objective_status: ObjectiveStatus,
) -> Option<(
    ObjectiveProgressKind,
    ObjectiveProgressPhase,
    ObjectiveStatus,
)> {
    Some(match event_type {
        "objective.planning_started" => (
            ObjectiveProgressKind::PlanningStarted,
            ObjectiveProgressPhase::Planning,
            ObjectiveStatus::Planning,
        ),
        "objective.plan_accepted" | "objective.planning_completed" => (
            ObjectiveProgressKind::PlanningCompleted,
            ObjectiveProgressPhase::Ready,
            ObjectiveStatus::Ready,
        ),
        "objective.plan_rejected" => (
            ObjectiveProgressKind::PlanRejected,
            ObjectiveProgressPhase::Failed,
            ObjectiveStatus::Failed,
        ),
        "objective.supervision_started" => (
            ObjectiveProgressKind::SupervisionStarted,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
        ),
        "objective.worker_started" => (
            ObjectiveProgressKind::WorkerStarted,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
        ),
        "objective.worker_completed" => (
            ObjectiveProgressKind::WorkerCompleted,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
        ),
        "objective.ticket_detected" => (
            ObjectiveProgressKind::TicketDetected,
            ObjectiveProgressPhase::Blocked,
            ObjectiveStatus::Blocked,
        ),
        "objective.ticket_resolution_started" => (
            ObjectiveProgressKind::TicketResolutionStarted,
            ObjectiveProgressPhase::Resolving,
            ObjectiveStatus::Running,
        ),
        "objective.ticket_resolution_completed" => (
            ObjectiveProgressKind::TicketResolutionCompleted,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
        ),
        "objective.worker_resumed" => (
            ObjectiveProgressKind::WorkerResumed,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
        ),
        "objective.validation_started" => (
            ObjectiveProgressKind::ValidationStarted,
            ObjectiveProgressPhase::Validating,
            ObjectiveStatus::Running,
        ),
        "objective.validation_failed" => (
            ObjectiveProgressKind::ValidationFailed,
            ObjectiveProgressPhase::Validating,
            ObjectiveStatus::Running,
        ),
        "objective.repair_task_created" => (
            ObjectiveProgressKind::RepairTaskCreated,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
        ),
        "objective.validation_passed" => (
            ObjectiveProgressKind::ValidationPassed,
            ObjectiveProgressPhase::Validating,
            ObjectiveStatus::Running,
        ),
        "objective.completed" => (
            ObjectiveProgressKind::Completed,
            ObjectiveProgressPhase::Complete,
            ObjectiveStatus::Complete,
        ),
        "objective.blocked" => (
            ObjectiveProgressKind::Blocked,
            ObjectiveProgressPhase::Blocked,
            ObjectiveStatus::Blocked,
        ),
        "objective.failed" => (
            ObjectiveProgressKind::Failed,
            ObjectiveProgressPhase::Failed,
            ObjectiveStatus::Failed,
        ),
        "objective.cancel_requested" => (
            ObjectiveProgressKind::CancelRequested,
            ObjectiveProgressPhase::Cancelled,
            objective_status,
        ),
        "objective.cancelled" => (
            ObjectiveProgressKind::Cancelled,
            ObjectiveProgressPhase::Cancelled,
            ObjectiveStatus::Cancelled,
        ),
        _ => return None,
    })
}

fn legacy_command_like(input: &str) -> bool {
    let mut tokens = input.split_whitespace();
    let Some(root) = tokens.next() else {
        return false;
    };
    let second = tokens.next();
    match root {
        "task" => matches!(second, Some("create" | "list" | "get" | "run" | "cleanup")),
        "ticket" => matches!(second, Some("list" | "get" | "resolve")),
        "objective" => matches!(
            second,
            Some("start" | "list" | "get" | "plan" | "validate" | "supervise" | "cancel")
        ),
        "config" => matches!(second, Some("get" | "set")),
        "workspace" => matches!(second, Some("prune")),
        "resume" | "supervise" => {
            second.is_some_and(|token| token.starts_with("task_") || token.starts_with("--"))
        }
        "run" => {
            input.contains("--title") || input.contains("--goal") || input.contains("--validation")
        }
        "init" | "doctor" | "completions" | "version" => {
            second.is_none() || second.is_some_and(|token| token.starts_with("--"))
        }
        "help" => second.is_none(),
        _ => false,
    }
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

#[derive(Clone)]
struct ServiceCompletionState {
    service_factory: RuntimeServiceFactory,
}

impl ServiceCompletionState {
    fn new(service_factory: RuntimeServiceFactory) -> Self {
        Self { service_factory }
    }

    fn service(&self) -> Box<dyn crate::service::HarnessService> {
        (self.service_factory)()
    }
}

impl CompletionStateView for ServiceCompletionState {
    fn tasks_for_completion(
        &self,
        scope: TaskCompletionScope,
    ) -> HarnessResult<Vec<TaskCompletionItem>> {
        Ok(self
            .service()
            .list_tasks()?
            .into_iter()
            .filter(|task| scope.statuses.contains(&task.status.as_str()))
            .map(|task| TaskCompletionItem {
                id: task.id,
                status: task.status,
                title: task.title,
            })
            .collect())
    }

    fn tickets_for_completion(
        &self,
        scope: TicketCompletionScope,
    ) -> HarnessResult<Vec<TicketCompletionItem>> {
        Ok(self
            .service()
            .list_tickets()?
            .into_iter()
            .filter(|ticket| scope.statuses.contains(&ticket.status.as_str()))
            .filter(|ticket| {
                scope
                    .task_id
                    .as_ref()
                    .map_or(true, |task_id| &ticket.task_id == task_id)
            })
            .map(|ticket| TicketCompletionItem {
                id: ticket.id,
                task_id: ticket.task_id,
                run_id: ticket.run_id,
                status: ticket.status,
                summary: ticket.question,
            })
            .collect())
    }

    fn objectives_for_completion(
        &self,
        scope: ObjectiveCompletionScope,
    ) -> HarnessResult<Vec<ObjectiveCompletionItem>> {
        let status_filter = if scope.statuses.len() == 1 {
            Some(crate::domain::ObjectiveStatus::parse(scope.statuses[0])?)
        } else {
            None
        };

        Ok(self
            .service()
            .list_objectives(status_filter)?
            .into_iter()
            .filter(|objective| scope.statuses.contains(&objective.status.as_str()))
            .map(|objective| ObjectiveCompletionItem {
                id: objective.id,
                status: objective.status,
                title: objective.title,
            })
            .collect())
    }
}

pub fn run_tui(service_factory: RuntimeServiceFactory) -> CommandExit {
    match run_tui_inner(service_factory) {
        Ok(()) => CommandExit::success(),
        Err(err) => CommandExit::new(CommandStatus::Failed, 1, Some(err.to_string())),
    }
}

fn run_tui_inner(service_factory: RuntimeServiceFactory) -> HarnessResult<()> {
    let mut guard = TerminalCleanupGuard::enter(CrosstermTerminalMode)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(io_error("create TUI terminal"))?;
    let result = run_event_loop(&mut terminal, service_factory);
    terminal
        .show_cursor()
        .map_err(io_error("restore terminal cursor"))?;
    guard.leave()?;
    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    service_factory: RuntimeServiceFactory,
) -> HarnessResult<()> {
    let mut app = TuiApp::new(service_factory)?;
    loop {
        app.drain_runtime_events();
        terminal
            .draw(|frame| render_app(frame, &app.state))
            .map_err(io_error("draw TUI"))?;

        if app.exit_requested() {
            return Ok(());
        }

        if event::poll(POLL_INTERVAL).map_err(io_error("poll terminal events"))?
            && let Event::Key(key) = event::read().map_err(io_error("read terminal event"))?
            && let Some(key) = map_key_event(key)
        {
            app.handle_key(key);
        }
    }
}

fn map_key_event(event: event::KeyEvent) -> Option<KeyEvent> {
    let modifiers = if event.modifiers.contains(CrosstermMods::CONTROL) {
        KeyModifiers::CONTROL
    } else {
        KeyModifiers::empty()
    };
    let code = match event.code {
        CrosstermKeyCode::Char(ch) => KeyCode::Char(ch),
        CrosstermKeyCode::Enter => KeyCode::Enter,
        CrosstermKeyCode::Tab => KeyCode::Tab,
        CrosstermKeyCode::Backspace => KeyCode::Backspace,
        CrosstermKeyCode::Left => KeyCode::Left,
        CrosstermKeyCode::Right => KeyCode::Right,
        CrosstermKeyCode::Home => KeyCode::Home,
        CrosstermKeyCode::End => KeyCode::End,
        CrosstermKeyCode::PageUp => KeyCode::PageUp,
        CrosstermKeyCode::PageDown => KeyCode::PageDown,
        CrosstermKeyCode::Up => KeyCode::Up,
        CrosstermKeyCode::Down => KeyCode::Down,
        CrosstermKeyCode::Esc => KeyCode::Esc,
        _ => return None,
    };
    Some(KeyEvent { code, modifiers })
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> HarnessError {
    move |err| HarnessError::External(format!("{context}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HarnessResult;
    use crate::domain::{Task, TaskId, Ticket, TicketId};
    use crate::runtime::{
        CommandResult, ObjectivePromptInput, ObjectiveStartOptions, ResumeTaskOptions,
        SuperviseCreateOptions, SuperviseTaskOptions, TaskRunOptions, TicketResolveOptions,
    };
    use crate::service::HarnessService;
    use crate::workspace::{CommandOutput, CommandSpec};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OBJECTIVE_ID: &str = "objective_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn terminal_cleanup_guard_restores_on_normal_leave_and_drop() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        {
            let mut guard = TerminalCleanupGuard::enter(RecordingMode(calls.clone())).unwrap();
            guard.leave().unwrap();
        }
        assert_eq!(calls.lock().unwrap().as_slice(), ["enter", "leave"]);

        let calls = Arc::new(Mutex::new(Vec::new()));
        {
            let _guard = TerminalCleanupGuard::enter(RecordingMode(calls.clone())).unwrap();
        }
        assert_eq!(calls.lock().unwrap().as_slice(), ["enter", "leave"]);
    }

    #[test]
    fn prompt_is_disabled_while_runtime_command_runs_and_reenabled_on_finish() {
        let mut app = TuiApp::with_shell_runner(
            Arc::new(|| Box::new(SlowService)),
            FakeShellRunner::default(),
        );
        app.composer.set_text(format!("/supervise {TASK_ID}"));
        app.handle_command(ComposerCommand::SubmitOrApplySuggestion);

        assert!(app.state.composer.disabled);
        assert_eq!(app.state.activity, CommandActivity::Running);

        collect_until_idle(&mut app);

        assert!(!app.state.composer.disabled);
        assert_eq!(app.state.activity, CommandActivity::Idle);
    }

    #[test]
    fn ctrl_c_cancels_running_runtime_command() {
        let mut app = TuiApp::with_shell_runner(
            Arc::new(|| Box::new(SlowService)),
            FakeShellRunner::default(),
        );
        app.composer.set_text(format!("/supervise {TASK_ID}"));
        app.handle_command(ComposerCommand::SubmitOrApplySuggestion);
        app.handle_command(ComposerCommand::Interrupt);

        assert_eq!(app.state.activity, CommandActivity::Cancelling);
        collect_until_idle(&mut app);

        assert_eq!(app.state.activity, CommandActivity::Idle);
        assert!(app.state.transcript.entries().any(|entry| {
            entry
                .text
                .contains("cancellation acknowledged; resume with `harness supervise")
        }));
    }

    #[test]
    fn shell_escape_runs_through_runner_and_streams_transcript() {
        let runner = FakeShellRunner {
            output: CommandOutput {
                stdout: "ok\n".to_string(),
                stderr: String::new(),
                exit_code: Some(0),
                duration_ms: 1,
                timed_out: false,
                truncated: false,
                truncated_bytes: 0,
            },
            ..FakeShellRunner::default()
        };
        let specs = runner.specs.clone();
        let mut app = TuiApp::with_shell_runner(Arc::new(|| Box::new(FakeService)), runner);

        app.composer.set_text("! printf ok");
        app.handle_command(ComposerCommand::SubmitOrApplySuggestion);
        assert!(app.state.composer.disabled);
        collect_until_idle(&mut app);

        assert_eq!(specs.lock().unwrap()[0].command, "printf ok");
        assert!(!app.state.composer.disabled);
        assert!(
            app.state
                .transcript
                .entries()
                .any(|entry| entry.text.contains("ok"))
        );
    }

    #[test]
    fn ctrl_c_cancels_running_shell_escape_and_acknowledges_promptly() {
        let runner = FakeShellRunner {
            wait_for_cancel: true,
            ..FakeShellRunner::default()
        };
        let mut app = TuiApp::with_shell_runner(Arc::new(|| Box::new(FakeService)), runner);

        app.composer.set_text("! sleep 5");
        app.handle_command(ComposerCommand::SubmitOrApplySuggestion);
        app.handle_command(ComposerCommand::Interrupt);

        let started = Instant::now();
        collect_until_idle(&mut app);

        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(app.state.activity, CommandActivity::Idle);
        assert!(
            app.state
                .transcript
                .entries()
                .any(|entry| entry.text == "cancellation acknowledged")
        );
    }

    #[test]
    fn prompt_first_plain_text_starts_supervised_objective() {
        let mut app = TuiApp::with_shell_runner(
            Arc::new(|| Box::new(FakeService)),
            FakeShellRunner::default(),
        );
        app.repo = Some(PathBuf::from("/prompt-repo"));
        app.composer.set_text("Create a Rust CLI");
        app.handle_command(ComposerCommand::SubmitOrApplySuggestion);

        assert!(
            app.state
                .transcript
                .entries()
                .any(|entry| entry.text.contains("Create a Rust CLI"))
        );
        assert!(
            app.state
                .transcript
                .entries()
                .any(|entry| entry.text.contains("/prompt-repo"))
        );
    }

    #[test]
    fn slash_command_routes_existing_runtime_command() {
        let mut app = TuiApp::with_shell_runner(
            Arc::new(|| Box::new(FakeService)),
            FakeShellRunner::default(),
        );
        app.composer.set_text("/supervise ".to_string() + TASK_ID);
        app.handle_command(ComposerCommand::SubmitOrApplySuggestion);
        collect_until_idle(&mut app);

        assert!(
            app.state
                .transcript
                .entries()
                .any(|entry| entry.text.contains("inspecting"))
        );
        assert!(
            !app.state
                .transcript
                .entries()
                .any(|entry| entry.text.contains("starting objective"))
        );
    }

    #[test]
    fn plain_known_command_root_shows_compatibility_warning() {
        let mut app = TuiApp::with_shell_runner(
            Arc::new(|| Box::new(FakeService)),
            FakeShellRunner::default(),
        );
        app.composer.set_text("task list");
        app.handle_command(ComposerCommand::Complete);

        assert!(
            app.state
                .transcript
                .entries()
                .any(|entry| entry.text.contains("use / for harness commands"))
                || app
                    .state
                    .composer
                    .hint
                    .as_deref()
                    .is_some_and(|hint| hint.contains("Use /task"))
        );
    }

    #[test]
    fn dashboard_hydrates_from_persisted_objective_events() {
        let objective = sample_objective(ObjectiveStatus::Running);
        let mut dashboard = DashboardPaneSnapshot::default();
        let service = ObjectiveHistoryService {
            objective: objective.clone(),
        };

        hydrate_objective_dashboard(&service, &mut dashboard, &objective);

        let objective = dashboard.objective.as_ref().unwrap();
        assert_eq!(objective.objective.phase, "resolving");
        assert!(
            matches!(&objective.remote_activity, PaneSection::Ready(rows) if rows.iter().any(|row| row.id == "planner" && row.status == "accepted"))
        );
        assert!(
            matches!(&objective.tickets, PaneSection::Ready(rows) if rows.iter().any(|row| row.id == TICKET_ID && row.status == "resolving"))
        );
    }

    fn collect_until_idle(app: &mut TuiApp<FakeShellRunner>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            app.drain_runtime_events();
            if app.state.activity == CommandActivity::Idle {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("app did not become idle");
    }

    struct RecordingMode(Arc<Mutex<Vec<&'static str>>>);

    impl TerminalMode for RecordingMode {
        fn enter(&mut self) -> HarnessResult<()> {
            self.0.lock().unwrap().push("enter");
            Ok(())
        }

        fn leave(&mut self) -> HarnessResult<()> {
            self.0.lock().unwrap().push("leave");
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeShellRunner {
        output: CommandOutput,
        specs: Arc<Mutex<Vec<CommandSpec>>>,
        wait_for_cancel: bool,
    }

    impl Default for FakeShellRunner {
        fn default() -> Self {
            Self {
                output: CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    duration_ms: 1,
                    timed_out: false,
                    truncated: false,
                    truncated_bytes: 0,
                },
                specs: Arc::new(Mutex::new(Vec::new())),
                wait_for_cancel: false,
            }
        }
    }

    impl ShellEscapeRunner for FakeShellRunner {
        fn run_shell_escape(
            &self,
            spec: CommandSpec,
            cancellation: &dyn crate::runtime::CancellationToken,
        ) -> HarnessResult<crate::tui::shell_escape::ShellEscapeOutput> {
            self.specs.lock().unwrap().push(spec);
            if self.wait_for_cancel {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !cancellation.is_cancelled() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Ok(crate::tui::shell_escape::ShellEscapeOutput::cancelled(
                    self.output.clone(),
                ));
            }
            Ok(crate::tui::shell_escape::ShellEscapeOutput::completed(
                self.output.clone(),
            ))
        }
    }

    struct FakeService;

    impl HarnessService for FakeService {
        fn create_task(
            &self,
            _title: String,
            _goal: String,
            _validation_commands: Vec<String>,
        ) -> HarnessResult<Task> {
            unreachable!()
        }

        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            Ok(Vec::new())
        }

        fn get_task(&self, _task_id: &TaskId) -> HarnessResult<Task> {
            unreachable!()
        }

        fn run_task(
            &self,
            _task_id: &TaskId,
            _options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            Ok(Vec::new())
        }

        fn get_ticket(&self, _ticket_id: &TicketId) -> HarnessResult<Ticket> {
            unreachable!()
        }

        fn resolve_ticket(
            &self,
            _ticket_id: &TicketId,
            _options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn resume_task(
            &self,
            _task_id: &TaskId,
            _options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn supervise_task(
            &self,
            task_id: &TaskId,
            _options: SuperviseTaskOptions,
        ) -> HarnessResult<CommandResult> {
            Ok(CommandResult::new(CommandExit::success()).with_event(
                crate::runtime::CommandEvent::supervise_progress(
                    crate::runtime::SuperviseProgressEvent {
                        phase: crate::runtime::SuperviseProgressPhase::InspectTask,
                        task_id: Some(task_id.clone()),
                        run_id: None,
                        ticket_id: None,
                        cycle: Some(0),
                        message: "inspecting".to_string(),
                        next_command: Some(format!("harness supervise {task_id}")),
                    },
                    crate::runtime::CommandEventLevel::Info,
                ),
            ))
        }

        fn create_and_supervise_task(
            &self,
            _options: SuperviseCreateOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn start_objective(&self, options: ObjectiveStartOptions) -> HarnessResult<CommandResult> {
            let objective_id = crate::domain::ObjectiveId::parse(OBJECTIVE_ID).unwrap();
            let prompt = match options.prompt {
                ObjectivePromptInput::Text(text) => text,
                ObjectivePromptInput::File(path) => path.to_string_lossy().into_owned(),
                ObjectivePromptInput::Stdin => "stdin".to_string(),
            };
            Ok(CommandResult::with_data(
                CommandExit::success(),
                serde_json::json!({
                    "objective_id": objective_id.as_str(),
                    "prompt": prompt,
                    "supervise": options.supervise,
                    "repo": options.runtime.repo.as_ref().map(|path| path.to_string_lossy().to_string()),
                }),
            ))
        }
    }

    struct ObjectiveHistoryService {
        objective: Objective,
    }

    impl HarnessService for ObjectiveHistoryService {
        fn create_task(
            &self,
            _title: String,
            _goal: String,
            _validation_commands: Vec<String>,
        ) -> HarnessResult<Task> {
            unreachable!()
        }

        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            Ok(Vec::new())
        }

        fn list_objectives(
            &self,
            _status: Option<ObjectiveStatus>,
        ) -> HarnessResult<Vec<Objective>> {
            Ok(vec![self.objective.clone()])
        }

        fn get_objective(
            &self,
            _objective_id: &crate::domain::ObjectiveId,
        ) -> HarnessResult<Objective> {
            Ok(self.objective.clone())
        }

        fn get_task(&self, _task_id: &TaskId) -> HarnessResult<Task> {
            unreachable!()
        }

        fn run_task(
            &self,
            _task_id: &TaskId,
            _options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            Ok(Vec::new())
        }

        fn get_ticket(&self, _ticket_id: &TicketId) -> HarnessResult<Ticket> {
            unreachable!()
        }

        fn resolve_ticket(
            &self,
            _ticket_id: &TicketId,
            _options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn resume_task(
            &self,
            _task_id: &TaskId,
            _options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            unreachable!()
        }

        fn get_objective_plan(
            &self,
            _objective_id: &crate::domain::ObjectiveId,
        ) -> HarnessResult<CommandResult> {
            Ok(CommandResult::with_data(
                CommandExit::success(),
                serde_json::json!({
                    "events": [
                        {
                            "event_type": "objective.plan_accepted",
                            "message": "objective plan accepted",
                            "created_at": "2026-05-14T00:00:01Z",
                            "payload_json": "{}"
                        },
                        {
                            "event_type": "objective.ticket_resolution_started",
                            "message": "resolving ticket",
                            "created_at": "2026-05-14T00:00:02Z",
                            "payload_json": serde_json::json!({
                                "task_id": TASK_ID,
                                "ticket_id": TICKET_ID
                            }).to_string()
                        }
                    ]
                }),
            ))
        }
    }

    fn sample_objective(status: ObjectiveStatus) -> Objective {
        Objective {
            id: crate::domain::ObjectiveId::parse(OBJECTIVE_ID).unwrap(),
            title: "Build CLI".to_string(),
            prompt: "Build CLI".to_string(),
            summary: "Build a CLI".to_string(),
            status,
            planner_model: Some("planner".to_string()),
            worker_model: Some("worker".to_string()),
            ticket_model: Some("ticket".to_string()),
            active_plan_id: None,
            monitor_lease_owner: None,
            monitor_lease_expires_at: None,
            created_at: "2026-05-14T00:00:00Z".to_string(),
            updated_at: "2026-05-14T00:00:03Z".to_string(),
        }
    }

    struct SlowService;

    impl HarnessService for SlowService {
        fn supervise_task(
            &self,
            task_id: &TaskId,
            options: SuperviseTaskOptions,
        ) -> HarnessResult<CommandResult> {
            std::thread::sleep(Duration::from_millis(40));
            FakeService.supervise_task(task_id, options)
        }

        fn create_task(&self, a: String, b: String, c: Vec<String>) -> HarnessResult<Task> {
            FakeService.create_task(a, b, c)
        }
        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            FakeService.list_tasks()
        }
        fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
            FakeService.get_task(task_id)
        }
        fn run_task(
            &self,
            task_id: &TaskId,
            options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.run_task(task_id, options)
        }
        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            FakeService.list_tickets()
        }
        fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
            FakeService.get_ticket(ticket_id)
        }
        fn resolve_ticket(
            &self,
            ticket_id: &TicketId,
            options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.resolve_ticket(ticket_id, options)
        }
        fn resume_task(
            &self,
            task_id: &TaskId,
            options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            FakeService.resume_task(task_id, options)
        }
    }
}
