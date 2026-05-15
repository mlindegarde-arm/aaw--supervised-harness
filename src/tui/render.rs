use crate::runtime::PaneSection;
use crate::tui::app_state::{TuiAppState, TuiFocus};
use crate::tui::composer::SuggestionRow;
use crate::tui::panes::{ObjectivePaneSnapshot, PaneRowView, SidePaneState};
use crate::tui::theme::TuiTheme;
use crate::tui::transcript::{TranscriptEntry, TranscriptSource};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const HEADER_HEIGHT: u16 = 1;
const COMPOSER_HEIGHT: u16 = 10;
const FOOTER_HEIGHT: u16 = 1;
const WIDE_MIN_WIDTH: u16 = 100;
const SIDE_PANE_WIDTH: u16 = 38;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Narrow,
    Wide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiLayout {
    pub mode: LayoutMode,
    pub header: Rect,
    pub transcript: Rect,
    pub side_pane: Rect,
    pub composer: Rect,
    pub footer: Rect,
}

pub fn compute_layout(area: Rect) -> TuiLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(6),
            Constraint::Length(COMPOSER_HEIGHT),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .split(area);

    let body = vertical[1];
    let mode = if area.width >= WIDE_MIN_WIDTH {
        LayoutMode::Wide
    } else {
        LayoutMode::Narrow
    };

    let (transcript, side_pane) = match mode {
        LayoutMode::Wide => {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(40), Constraint::Length(SIDE_PANE_WIDTH)])
                .split(body);
            (chunks[0], chunks[1])
        }
        LayoutMode::Narrow => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
                .split(body);
            (chunks[0], chunks[1])
        }
    };

    TuiLayout {
        mode,
        header: vertical[0],
        transcript,
        side_pane,
        composer: vertical[2],
        footer: vertical[3],
    }
}

pub fn render_app(frame: &mut Frame<'_>, app: &TuiAppState) {
    let theme = TuiTheme::default();
    let layout = compute_layout(frame.area());

    frame.render_widget(render_header(app, &theme), layout.header);
    frame.render_widget(
        render_transcript(app, &theme, app.focus == TuiFocus::Transcript),
        layout.transcript,
    );
    let mut side_pane_state =
        ListState::default().with_offset(app.panes.active_state().scroll as usize);
    frame.render_stateful_widget(
        render_side_pane(&app.panes, &theme, app.focus == TuiFocus::SidePane),
        layout.side_pane,
        &mut side_pane_state,
    );
    frame.render_widget(
        render_composer(app, &theme, layout.composer),
        layout.composer,
    );
    frame.render_widget(render_footer(app, &theme), layout.footer);
    if let Some(position) = composer_cursor_position(app, layout.composer) {
        frame.set_cursor_position(position);
    }
}

fn render_header<'a>(app: &'a TuiAppState, theme: &TuiTheme) -> Paragraph<'a> {
    let active_task = app.header.active_task.as_deref().unwrap_or("-");
    let active_run = app.header.active_run.as_deref().unwrap_or("-");
    let text = format!(
        "repo {} | model {} | ticket {} | task {} | run {} | open tickets {} | {}",
        app.header.repo,
        app.header.local_model,
        app.header.ticket_model,
        active_task,
        active_run,
        app.header.open_tickets,
        app.header.phase
    );
    Paragraph::new(text).style(theme.header)
}

fn render_footer<'a>(app: &'a TuiAppState, theme: &TuiTheme) -> Paragraph<'a> {
    let hints = app.footer.hints.join(" | ");
    Paragraph::new(format!(
        "{} | {} | {}",
        app.footer.mode, app.footer.status, hints
    ))
    .style(theme.muted)
}

fn render_composer<'a>(app: &'a TuiAppState, theme: &TuiTheme, area: Rect) -> Paragraph<'a> {
    let style = if app.composer.disabled {
        theme.muted
    } else {
        theme.base
    };

    let inner_height = area.height.saturating_sub(1) as usize;
    let wrapped_input = wrap_composer_input(app, area);
    let mut lines: Vec<Line<'_>> = wrapped_input
        .visible_lines
        .iter()
        .map(|line| Line::styled(line.clone(), style))
        .collect();

    if !app.composer.disabled {
        let remaining_lines = inner_height.saturating_sub(lines.len());
        lines.extend(
            app.composer
                .suggestions
                .iter()
                .take(remaining_lines)
                .map(|row| suggestion_line(row, theme)),
        );
        if app.composer.suggestions.is_empty()
            && let Some(hint) = &app.composer.hint
            && lines.len() < inner_height
        {
            lines.push(Line::styled(format!("  {hint}"), theme.muted));
        }
    }
    let title = if app.composer.disabled {
        "Composer (command running)"
    } else {
        "Composer"
    };
    Paragraph::new(lines).block(Block::default().borders(Borders::TOP).title(title))
}

fn composer_cursor_position(app: &TuiAppState, area: Rect) -> Option<Position> {
    if app.focus != TuiFocus::Composer
        || app.composer.disabled
        || area.width == 0
        || area.height < 2
    {
        return None;
    }

    let wrapped_input = wrap_composer_input(app, area);
    let visible_cursor_row = wrapped_input
        .cursor_row
        .saturating_sub(wrapped_input.first_visible_row);
    if visible_cursor_row >= area.height.saturating_sub(1) as usize {
        return None;
    }

    Some(Position {
        x: area.x
            + wrapped_input
                .cursor_col
                .min(area.width.saturating_sub(1) as usize) as u16,
        y: area.y + 1 + visible_cursor_row as u16,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrappedComposerInput {
    visible_lines: Vec<String>,
    first_visible_row: usize,
    cursor_row: usize,
    cursor_col: usize,
}

fn wrap_composer_input(app: &TuiAppState, area: Rect) -> WrappedComposerInput {
    let width = usize::from(area.width).max(1);
    let visible_height = usize::from(area.height.saturating_sub(1)).max(1);
    let prompt = app.composer.prompt.as_str();
    let continuation_prefix = " ".repeat(display_width(prompt));
    let cursor = clamp_to_char_boundary(&app.composer.text, app.composer.cursor);
    let mut lines = Vec::new();
    let mut line = prompt.to_string();
    let mut line_width = display_width(prompt);
    let mut row = 0usize;
    let mut cursor_row = 0usize;
    let mut cursor_col = line_width;
    let mut cursor_recorded = false;

    for (idx, ch) in app.composer.text.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if line_width.saturating_add(ch_width) > width && line_width > 0 {
            lines.push(line);
            line = continuation_prefix.clone();
            line_width = display_width(&line);
            row += 1;
        }
        if idx == cursor {
            cursor_row = row;
            cursor_col = line_width;
            cursor_recorded = true;
        }
        line.push(ch);
        line_width = line_width.saturating_add(ch_width);
    }

    if !cursor_recorded {
        if line_width >= width && cursor == app.composer.text.len() {
            lines.push(line);
            line = continuation_prefix;
            line_width = display_width(&line);
            row += 1;
        }
        cursor_row = row;
        cursor_col = line_width;
    }

    lines.push(line);
    let first_visible_row = cursor_row.saturating_sub(visible_height.saturating_sub(1));
    let visible_lines = lines
        .iter()
        .skip(first_visible_row)
        .take(visible_height)
        .cloned()
        .collect();

    WrappedComposerInput {
        visible_lines,
        first_visible_row,
        cursor_row,
        cursor_col,
    }
}

fn suggestion_line<'a>(row: &'a SuggestionRow, theme: &TuiTheme) -> Line<'a> {
    let prefix = if row.selected { "> " } else { "  " };
    let style = if row.selected {
        theme.selected
    } else {
        theme.base
    };
    let detail = if row.detail.is_empty() {
        String::new()
    } else {
        format!("  {}", truncate(&row.detail, 44))
    };
    Line::from(vec![
        Span::styled(prefix, style),
        Span::styled(format!("{:<64}", truncate(&row.display, 64)), style),
        Span::styled(detail, theme.muted),
    ])
}

fn render_transcript<'a>(app: &'a TuiAppState, theme: &TuiTheme, focused: bool) -> Paragraph<'a> {
    let lines = if app.transcript.is_empty() {
        vec![Line::styled("No activity yet", theme.muted)]
    } else {
        app.transcript
            .entries()
            .flat_map(|entry| transcript_entry_lines(entry, theme))
            .collect()
    };
    let block = focused_block("Transcript", focused, theme);
    Paragraph::new(lines)
        .block(block)
        .scroll((app.transcript.scroll_from_bottom(), 0))
        .wrap(Wrap { trim: false })
}

fn transcript_entry_lines<'a>(entry: &'a TranscriptEntry, theme: &TuiTheme) -> Vec<Line<'a>> {
    let style = match entry.source {
        TranscriptSource::Stdout => theme.stdout,
        TranscriptSource::Stderr | TranscriptSource::Error => theme.stderr,
        TranscriptSource::Command => theme.base,
        TranscriptSource::Progress => theme.warning,
        TranscriptSource::System => theme.muted,
    };
    entry
        .text
        .lines()
        .map(|line| {
            Line::from(vec![
                Span::styled(
                    format!("[{}] ", entry.source.label()),
                    theme.muted.add_modifier(Modifier::BOLD),
                ),
                Span::styled(line, style),
            ])
        })
        .collect()
}

fn render_side_pane<'a>(panes: &'a SidePaneState, theme: &TuiTheme, focused: bool) -> List<'a> {
    if let Some(objective) = panes.objective() {
        return render_objective_dashboard(objective, theme, focused);
    }

    let title = format!(
        "{}{}",
        panes.active().title(),
        if panes.refresh().stale { " *" } else { "" }
    );
    let items = match panes.pane_message(panes.active()) {
        Some(message) => vec![ListItem::new(Line::styled(message, theme.muted))],
        None => panes
            .rows_for(panes.active())
            .into_iter()
            .enumerate()
            .map(|(idx, row)| side_pane_item(idx, row, panes.active_state().selected, theme))
            .collect(),
    };

    List::new(items).block(focused_block(title, focused, theme))
}

fn render_objective_dashboard<'a>(
    objective: &'a ObjectivePaneSnapshot,
    theme: &TuiTheme,
    focused: bool,
) -> List<'a> {
    let mut items = Vec::new();
    items.push(ListItem::new(Line::from(vec![
        Span::styled("Objective ", theme.muted.add_modifier(Modifier::BOLD)),
        Span::styled(truncate(&objective.objective.id, 16), theme.base),
    ])));
    items.push(ListItem::new(Line::from(vec![
        Span::styled(
            format!("{:<10}", objective.objective.phase),
            lifecycle_style(&objective.objective.phase, theme),
        ),
        Span::styled(
            format!(" {}", truncate(&objective.objective.message, 58)),
            theme.base,
        ),
    ])));
    push_objective_section(&mut items, "Tasks", &objective.tasks, theme);
    push_objective_section(&mut items, "Workers", &objective.workers, theme);
    push_objective_section(&mut items, "Tickets", &objective.tickets, theme);
    push_objective_section(&mut items, "Validation", &objective.validation, theme);
    push_objective_section(&mut items, "Remote", &objective.remote_activity, theme);
    push_transcript_section(&mut items, &objective.transcript, theme);

    List::new(items).block(focused_block("Objective Dashboard", focused, theme))
}

trait DashboardRow {
    fn row_id(&self) -> &str;
    fn row_status(&self) -> &str;
    fn row_detail(&self) -> &str;
}

macro_rules! impl_dashboard_row {
    ($type:ty, $detail:ident) => {
        impl DashboardRow for $type {
            fn row_id(&self) -> &str {
                &self.id
            }

            fn row_status(&self) -> &str {
                &self.status
            }

            fn row_detail(&self) -> &str {
                &self.$detail
            }
        }
    };
}

impl_dashboard_row!(crate::tui::panes::AcceptanceCriterionRow, description);
impl_dashboard_row!(crate::tui::panes::ObjectiveTaskRow, detail);
impl_dashboard_row!(crate::tui::panes::ObjectiveWorkerRow, detail);
impl_dashboard_row!(crate::tui::panes::ObjectiveTicketRow, detail);
impl_dashboard_row!(crate::tui::panes::ObjectiveValidationRow, detail);
impl_dashboard_row!(crate::tui::panes::PlannerExchangeRow, detail);

fn push_objective_section<'a, T>(
    items: &mut Vec<ListItem<'a>>,
    title: &'static str,
    section: &'a PaneSection<T>,
    theme: &TuiTheme,
) where
    T: DashboardRow,
{
    let rows = match section {
        PaneSection::Ready(rows) => rows,
        PaneSection::Loading => {
            items.push(ListItem::new(Line::styled(
                format!("{title}: loading"),
                theme.muted,
            )));
            return;
        }
        PaneSection::Error(message) => {
            items.push(ListItem::new(Line::styled(
                format!("{title}: error {}", truncate(message, 48)),
                theme.stderr,
            )));
            return;
        }
    };
    items.push(ListItem::new(Line::styled(
        title,
        theme.muted.add_modifier(Modifier::BOLD),
    )));
    if rows.is_empty() {
        items.push(ListItem::new(Line::styled("  none", theme.muted)));
        return;
    }
    items.extend(rows.iter().take(4).map(|row| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("  {:<10}", truncate(row.row_status(), 10)),
                lifecycle_style(row.row_status(), theme),
            ),
            Span::styled(format!("{:<14}", truncate(row.row_id(), 14)), theme.base),
            Span::styled(truncate(row.row_detail(), 44), theme.muted),
        ]))
    }));
}

fn push_transcript_section<'a>(
    items: &mut Vec<ListItem<'a>>,
    section: &'a PaneSection<crate::tui::panes::ObjectiveTranscriptRow>,
    theme: &TuiTheme,
) {
    let PaneSection::Ready(rows) = section else {
        return;
    };
    items.push(ListItem::new(Line::styled(
        "Recent",
        theme.muted.add_modifier(Modifier::BOLD),
    )));
    if rows.is_empty() {
        items.push(ListItem::new(Line::styled("  none", theme.muted)));
        return;
    }
    items.extend(rows.iter().take(4).map(|row| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("  {:<10}", truncate(&row.phase, 10)),
                lifecycle_style(&row.phase, theme),
            ),
            Span::styled(truncate(&row.message, 56), theme.base),
        ]))
    }));
}

fn lifecycle_style(status: &str, theme: &TuiTheme) -> ratatui::style::Style {
    match status {
        "complete" | "passed" | "passing" | "accepted" | "resolved" => theme.stdout,
        "failed" | "blocked" | "cancelled" | "rejected" | "failing" => theme.stderr,
        "running" | "resolving" | "validating" | "planning" | "resuming" => theme.warning,
        _ => theme.base,
    }
}

fn side_pane_item(
    idx: usize,
    row: PaneRowView,
    selected: usize,
    theme: &TuiTheme,
) -> ListItem<'static> {
    let style = if idx == selected {
        theme.selected
    } else {
        theme.base
    };
    let line = Line::from(vec![
        Span::styled(format!("{:<12}", truncate(&row.id, 12)), style),
        Span::styled(format!(" {:<10} ", truncate(&row.status, 10)), style),
        Span::styled(truncate(&row.detail, 80), style),
    ]);
    ListItem::new(line)
}

fn focused_block(title: impl Into<String>, focused: bool, theme: &TuiTheme) -> Block<'static> {
    let border_style = if focused {
        theme.focused_border
    } else {
        theme.border
    };
    Block::default()
        .borders(Borders::ALL)
        .title(title.into())
        .border_style(border_style)
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut output = String::new();
    for _ in 0..max_chars {
        match chars.next() {
            Some(ch) => output.push(ch),
            None => return output,
        }
    }
    if chars.next().is_some() {
        while output.chars().count() + 3 > max_chars && !output.is_empty() {
            output.pop();
        }
        output.push_str("...");
    }
    output
}

fn display_width(text: &str) -> usize {
    text.width()
}

fn clamp_to_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[allow(dead_code)]
fn section_len<T>(section: &PaneSection<T>) -> usize {
    match section {
        PaneSection::Ready(rows) => rows.len(),
        PaneSection::Loading | PaneSection::Error(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ArtifactId, ObjectiveId, ObjectiveStatus, RunId, RunStatus, TaskId, TaskStatus, TicketId,
        TicketStatus,
    };
    use crate::runtime::{
        ObjectiveProgressEvent, ObjectiveProgressKind, ObjectiveProgressPhase, PaneArtifactRow,
        PaneRunRow, PaneSection, PaneStateSnapshot, PaneTaskRow, PaneTicketRow,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ARTIFACT_ID: &str = "art_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OBJECTIVE_ID: &str = "objective_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn render_layout_uses_wide_side_by_side_mode() {
        let layout = compute_layout(Rect::new(0, 0, 120, 40));

        assert_eq!(layout.mode, LayoutMode::Wide);
        assert_eq!(layout.side_pane.x, 82);
        assert_eq!(layout.side_pane.width, 38);
        assert_eq!(layout.transcript.y, layout.side_pane.y);
    }

    #[test]
    fn render_layout_uses_narrow_stacked_mode() {
        let layout = compute_layout(Rect::new(0, 0, 80, 30));

        assert_eq!(layout.mode, LayoutMode::Narrow);
        assert_eq!(layout.transcript.x, layout.side_pane.x);
        assert!(layout.side_pane.y > layout.transcript.y);
        assert_eq!(layout.composer.height, COMPOSER_HEIGHT);
    }

    #[test]
    fn render_empty_loading_and_error_states_without_terminal() {
        let mut app = TuiAppState::default();
        app.set_pane_snapshot(PaneStateSnapshot {
            tasks: PaneSection::Ready(Vec::new()),
            tickets: PaneSection::Error("ticket store down".to_string()),
            runs: PaneSection::Loading,
            artifacts: PaneSection::Ready(Vec::new()),
        });

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("No activity yet"));
        assert!(buffer.contains("no rows"));

        app.panes.next_pane();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());
        assert!(buffer.contains("error: ticket store down"));
    }

    #[test]
    fn render_sanitized_transcript_text() {
        let mut app = TuiAppState::default();
        app.transcript.append_untrusted_text(
            crate::tui::transcript::TranscriptSource::Stdout,
            "hello\x1b[2J OPENAI_API_KEY=sk-test-secret",
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("hello"));
        assert!(buffer.contains("[REDACTED"));
        assert!(!buffer.contains("sk-test-secret"));
        assert!(!buffer.contains("\\u{1b}"));
    }

    #[test]
    fn render_pane_rows_with_selection_marker_style() {
        let mut app = TuiAppState::default();
        app.set_pane_snapshot(snapshot_with_rows());
        app.panes.select_next();

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("Run tests"));
        assert!(buffer.contains("running"));
    }

    #[test]
    fn tui_dashboard_renders_objective_lifecycle_panels() {
        let mut app = TuiAppState::default();
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let ticket_id = TicketId::parse(TICKET_ID).unwrap();
        app.panes.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::PlanningStarted,
            ObjectiveProgressPhase::Planning,
            "planning objective",
            None,
            None,
        ));
        app.panes.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::WorkerStarted,
            ObjectiveProgressPhase::Running,
            "running local worker",
            Some(task_id.clone()),
            None,
        ));
        app.panes.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::TicketResolutionStarted,
            ObjectiveProgressPhase::Resolving,
            "remote resolver running",
            Some(task_id),
            Some(ticket_id),
        ));
        app.panes.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::ValidationStarted,
            ObjectiveProgressPhase::Validating,
            "validation running",
            None,
            None,
        ));

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("Objective Dashboard"));
        assert!(buffer.contains("Workers"));
        assert!(buffer.contains("Tickets"));
        assert!(buffer.contains("Validation"));
        assert!(buffer.contains("Remote"));
        assert!(buffer.contains("remote resolver running"));
    }

    #[test]
    fn tui_dashboard_narrow_layout_keeps_lifecycle_visible() {
        let mut app = TuiAppState::default();
        app.panes.apply_objective_progress(&objective_event(
            ObjectiveProgressKind::Completed,
            ObjectiveProgressPhase::Complete,
            "objective complete",
            None,
            None,
        ));

        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("Objective Dashboard"));
        assert!(buffer.contains("complete"));
        assert!(buffer.contains("objective complete"));
    }

    #[test]
    fn render_side_pane_honors_scroll_offset() {
        let mut app = TuiAppState::default();
        app.set_pane_snapshot(snapshot_with_task_rows(14));
        app.panes.scroll_down(10);

        let mut terminal = Terminal::new(TestBackend::new(120, 16)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("Task row 10"));
        assert!(!buffer.contains("Task row 02"));
    }

    #[test]
    fn render_composer_includes_text_after_cursor() {
        let mut app = TuiAppState::default();
        app.composer.text = "before after".to_string();
        app.composer.cursor = "before".len();

        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("> before after"));
    }

    #[test]
    fn render_sets_terminal_cursor_to_composer_cursor() {
        let mut app = TuiAppState::default();
        app.composer.text = "before after".to_string();
        app.composer.cursor = "before".len();
        let layout = compute_layout(Rect::new(0, 0, 80, 24));

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();

        terminal.backend_mut().assert_cursor_position(Position {
            x: layout.composer.x + "> before".len() as u16,
            y: layout.composer.y + 1,
        });
    }

    #[test]
    fn render_wraps_long_composer_text_and_moves_cursor_to_next_row() {
        let mut app = TuiAppState::default();
        app.composer.text = "abcdefghijklmnopqrstu".to_string();
        app.composer.cursor = app.composer.text.len();
        let layout = compute_layout(Rect::new(0, 0, 20, 24));

        let mut terminal = Terminal::new(TestBackend::new(20, 24)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();
        let buffer = format!("{:?}", terminal.backend().buffer());

        assert!(buffer.contains("> abcdefghijklmnopqr"));
        assert!(buffer.contains("  stu"));
        terminal.backend_mut().assert_cursor_position(Position {
            x: layout.composer.x + 5,
            y: layout.composer.y + 2,
        });
    }

    #[test]
    fn render_keeps_cursor_visible_for_very_long_composer_text() {
        let mut app = TuiAppState::default();
        app.composer.text = "x".repeat(120);
        app.composer.cursor = app.composer.text.len();
        let layout = compute_layout(Rect::new(0, 0, 12, 24));

        let mut terminal = Terminal::new(TestBackend::new(12, 24)).unwrap();
        terminal.draw(|frame| render_app(frame, &app)).unwrap();

        terminal.backend_mut().assert_cursor_position(Position {
            x: layout.composer.x + 2,
            y: layout.composer.y + layout.composer.height - 1,
        });
    }

    fn snapshot_with_rows() -> PaneStateSnapshot {
        let mut snapshot = snapshot_with_task_rows(2);
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let run_id = RunId::parse(RUN_ID).unwrap();
        snapshot.tickets = PaneSection::Ready(vec![PaneTicketRow {
            ticket_id: crate::domain::TicketId::parse(TICKET_ID).unwrap(),
            status: TicketStatus::Open,
            task_id: task_id.clone(),
            run_id: run_id.clone(),
            blocked_on: "ambiguous error".to_string(),
            question: "What should happen?".to_string(),
        }]);
        snapshot.runs = PaneSection::Ready(vec![PaneRunRow {
            run_id,
            task_id: task_id.clone(),
            status: RunStatus::Running,
            escalation_cycle: 1,
            current_phase: Some("validation".to_string()),
            latest_artifact_path: Some("logs/run.txt".to_string()),
        }]);
        snapshot.artifacts = PaneSection::Ready(vec![PaneArtifactRow {
            artifact_id: ArtifactId::parse(ARTIFACT_ID).unwrap(),
            kind: "log".to_string(),
            path: "logs/run.txt".to_string(),
            byte_len: 128,
            sha256_prefix: "abcd1234".to_string(),
            task_id,
            run_id: None,
            ticket_id: None,
        }]);
        snapshot
    }

    fn snapshot_with_task_rows(row_count: usize) -> PaneStateSnapshot {
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let run_id = RunId::parse(RUN_ID).unwrap();
        PaneStateSnapshot {
            tasks: PaneSection::Ready(
                (0..row_count)
                    .map(|idx| PaneTaskRow {
                        task_id: task_id.clone(),
                        status: if idx == 1 {
                            TaskStatus::Running
                        } else {
                            TaskStatus::Ready
                        },
                        title: if idx == 0 {
                            "Fix parser".to_string()
                        } else if idx == 1 {
                            "Run tests".to_string()
                        } else {
                            format!("Task row {idx:02}")
                        },
                        latest_run_id: (idx == 1).then(|| run_id.clone()),
                        updated_at: format!("2026-05-14T00:{idx:02}:00Z"),
                    })
                    .collect(),
            ),
            tickets: PaneSection::Ready(Vec::new()),
            runs: PaneSection::Ready(Vec::new()),
            artifacts: PaneSection::Ready(Vec::new()),
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
