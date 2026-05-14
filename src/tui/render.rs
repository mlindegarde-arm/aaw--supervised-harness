use crate::runtime::PaneSection;
use crate::tui::app_state::{TuiAppState, TuiFocus};
use crate::tui::composer::SuggestionRow;
use crate::tui::panes::{PaneRowView, SidePaneState};
use crate::tui::theme::TuiTheme;
use crate::tui::transcript::{TranscriptEntry, TranscriptSource};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

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
    frame.render_widget(render_composer(app, &theme), layout.composer);
    frame.render_widget(render_footer(app, &theme), layout.footer);
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

fn render_composer<'a>(app: &'a TuiAppState, theme: &TuiTheme) -> Paragraph<'a> {
    let style = if app.composer.disabled {
        theme.muted
    } else {
        theme.base
    };
    let mut lines = vec![Line::styled(
        format!("{}{}", app.composer.prompt, app.composer.text),
        style,
    )];
    if !app.composer.disabled {
        lines.extend(
            app.composer
                .suggestions
                .iter()
                .map(|row| suggestion_line(row, theme)),
        );
        if app.composer.suggestions.is_empty()
            && let Some(hint) = &app.composer.hint
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
    use crate::domain::{ArtifactId, RunId, RunStatus, TaskId, TaskStatus, TicketStatus};
    use crate::runtime::{
        PaneArtifactRow, PaneRunRow, PaneSection, PaneStateSnapshot, PaneTaskRow, PaneTicketRow,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ARTIFACT_ID: &str = "art_01ARZ3NDEKTSV4RRFFQ69G5FAV";

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
}
