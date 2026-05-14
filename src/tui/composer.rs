use crate::completion::{
    CommandReadiness, CompleterEngine, CompletionCandidate, CompletionContext, CompletionKind,
    CompletionSet, CompletionStatus,
};
use crate::error::HarnessResult;
use std::cmp;

const DEFAULT_MAX_SUGGESTIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxClass {
    Command,
    Option,
    Value,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    pub start: usize,
    pub end: usize,
    pub class: SyntaxClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionRow {
    pub replacement: String,
    pub display: String,
    pub detail: String,
    pub kind: CompletionKind,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerOutcome {
    Edited,
    AppliedSuggestion,
    Submitted(String),
    Blocked { hint: String },
    ExitRequested,
    Cleared,
    Noop,
}

#[derive(Debug, Clone)]
pub struct ComposerState {
    buffer: String,
    cursor: usize,
    selected_suggestion: Option<usize>,
    suggestions_visible: bool,
    completion_set: Option<CompletionSet>,
    history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
    max_suggestions: usize,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            selected_suggestion: None,
            suggestions_visible: false,
            completion_set: None,
            history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
            max_suggestions: DEFAULT_MAX_SUGGESTIONS,
        }
    }
}

impl ComposerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_suggestions(max_suggestions: usize) -> Self {
        Self {
            max_suggestions,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.buffer
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.buffer = text.into();
        self.cursor = self.buffer.len();
        self.reset_transient_state();
    }

    pub fn set_completion_set(&mut self, completion_set: CompletionSet) {
        self.selected_suggestion = None;
        self.suggestions_visible = !completion_set.candidates.is_empty();
        self.completion_set = Some(completion_set);
    }

    pub fn refresh_completion<E: CompleterEngine>(
        &mut self,
        engine: &E,
        context: &CompletionContext<'_>,
    ) -> HarnessResult<()> {
        let completion_set = engine.complete(&self.buffer, self.cursor, context)?;
        self.set_completion_set(completion_set);
        Ok(())
    }

    pub fn completion_status(&self) -> Option<&CompletionStatus> {
        self.completion_set.as_ref().map(|set| &set.status)
    }

    pub fn readiness(&self) -> Option<&CommandReadiness> {
        self.completion_set.as_ref().map(|set| &set.readiness)
    }

    pub fn readiness_hint(&self) -> Option<String> {
        match self.readiness()? {
            CommandReadiness::Ready => None,
            CommandReadiness::Incomplete { hint, .. } => Some(hint.clone()),
            CommandReadiness::Invalid { diagnostic } => Some(diagnostic.clone()),
        }
    }

    pub fn shell_escape_hint(&self) -> Option<String> {
        if !self.buffer.trim_start().starts_with('!') {
            return None;
        }
        self.completion_set
            .as_ref()
            .and_then(|set| set.hint.as_ref())
            .map(|hint| {
                if hint.detail.is_empty() {
                    hint.display.clone()
                } else {
                    format!("{}: {}", hint.display, hint.detail)
                }
            })
            .or_else(|| Some("shell escape: run from the repository root".to_string()))
    }

    pub fn hint_text(&self) -> Option<String> {
        self.shell_escape_hint()
            .or_else(|| {
                self.completion_set
                    .as_ref()
                    .and_then(|set| set.hint.as_ref())
                    .map(format_candidate_hint)
            })
            .or_else(|| self.readiness_hint())
    }

    pub fn suggestions_visible(&self) -> bool {
        self.suggestions_visible
    }

    pub fn selected_suggestion(&self) -> Option<usize> {
        self.selected_suggestion
    }

    pub fn suggestion_rows(&self) -> Vec<SuggestionRow> {
        let Some(set) = &self.completion_set else {
            return Vec::new();
        };
        if !self.suggestions_visible {
            return Vec::new();
        }
        let limit = cmp::min(self.max_suggestions, set.candidates.len());
        set.candidates
            .iter()
            .take(limit)
            .enumerate()
            .map(|(idx, candidate)| SuggestionRow {
                replacement: candidate.replacement.clone(),
                display: candidate.display.clone(),
                detail: candidate.detail.clone(),
                kind: candidate.kind,
                selected: self.selected_suggestion == Some(idx),
            })
            .collect()
    }

    pub fn syntax_tokens(&self) -> Vec<SyntaxToken> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        if matches!(
            self.readiness(),
            Some(CommandReadiness::Invalid { diagnostic: _ })
        ) {
            return vec![SyntaxToken {
                start: 0,
                end: self.buffer.len(),
                class: SyntaxClass::Error,
            }];
        }

        let mut tokens = Vec::new();
        for (idx, (start, end, text)) in split_shell_words(&self.buffer).into_iter().enumerate() {
            let class = if text.starts_with("--") {
                SyntaxClass::Option
            } else if idx == 0
                || tokens
                    .last()
                    .is_some_and(|token: &SyntaxToken| token.class == SyntaxClass::Command)
                    && !text.starts_with('-')
                    && !previous_nonspace_is_value_context(&self.buffer, start)
            {
                SyntaxClass::Command
            } else {
                SyntaxClass::Value
            };
            tokens.push(SyntaxToken { start, end, class });
        }
        tokens
    }

    pub fn insert_char(&mut self, ch: char) -> ComposerOutcome {
        self.leave_history();
        self.buffer.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.reset_completion_state();
        ComposerOutcome::Edited
    }

    pub fn move_left(&mut self) -> ComposerOutcome {
        if let Some(previous) = previous_boundary(&self.buffer, self.cursor) {
            self.cursor = previous;
            ComposerOutcome::Edited
        } else {
            ComposerOutcome::Noop
        }
    }

    pub fn move_right(&mut self) -> ComposerOutcome {
        if let Some(next) = next_boundary(&self.buffer, self.cursor) {
            self.cursor = next;
            ComposerOutcome::Edited
        } else {
            ComposerOutcome::Noop
        }
    }

    pub fn move_to_start(&mut self) -> ComposerOutcome {
        if self.cursor == 0 {
            ComposerOutcome::Noop
        } else {
            self.cursor = 0;
            ComposerOutcome::Edited
        }
    }

    pub fn move_to_end(&mut self) -> ComposerOutcome {
        if self.cursor == self.buffer.len() {
            ComposerOutcome::Noop
        } else {
            self.cursor = self.buffer.len();
            ComposerOutcome::Edited
        }
    }

    pub fn backspace(&mut self) -> ComposerOutcome {
        self.leave_history();
        let Some(previous) = previous_boundary(&self.buffer, self.cursor) else {
            return ComposerOutcome::Noop;
        };
        self.buffer.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        self.reset_completion_state();
        ComposerOutcome::Edited
    }

    pub fn clear_before_cursor(&mut self) -> ComposerOutcome {
        self.leave_history();
        if self.cursor == 0 {
            return ComposerOutcome::Noop;
        }
        self.buffer.replace_range(0..self.cursor, "");
        self.cursor = 0;
        self.reset_completion_state();
        ComposerOutcome::Edited
    }

    pub fn delete_previous_word(&mut self) -> ComposerOutcome {
        self.leave_history();
        let Some(delete_start) = previous_word_start(&self.buffer, self.cursor) else {
            return ComposerOutcome::Noop;
        };
        self.buffer.replace_range(delete_start..self.cursor, "");
        self.cursor = delete_start;
        self.reset_completion_state();
        ComposerOutcome::Edited
    }

    pub fn tab_complete(&mut self) -> ComposerOutcome {
        if self.apply_selected_if_visible() {
            return ComposerOutcome::AppliedSuggestion;
        }

        let Some(set) = self.completion_set.clone() else {
            return ComposerOutcome::Noop;
        };
        match set.candidates.as_slice() {
            [candidate] => {
                self.apply_candidate(candidate, set.replacement_start, set.replacement_end);
                ComposerOutcome::AppliedSuggestion
            }
            [] => {
                self.suggestions_visible = false;
                ComposerOutcome::Noop
            }
            _ => {
                if self.apply_longest_common_prefix(&set) {
                    ComposerOutcome::AppliedSuggestion
                } else {
                    self.suggestions_visible = true;
                    ComposerOutcome::Noop
                }
            }
        }
    }

    pub fn enter(&mut self) -> ComposerOutcome {
        if self.apply_selected_if_visible() {
            return ComposerOutcome::AppliedSuggestion;
        }

        let command = self.buffer.trim().to_string();
        if command.is_empty() {
            return ComposerOutcome::Noop;
        }
        if command.starts_with('!') {
            self.push_history(command.clone());
            self.buffer.clear();
            self.cursor = 0;
            self.reset_transient_state();
            return ComposerOutcome::Submitted(command);
        }
        match self.readiness().cloned().unwrap_or(CommandReadiness::Ready) {
            CommandReadiness::Ready => {
                self.push_history(command.clone());
                self.buffer.clear();
                self.cursor = 0;
                self.reset_transient_state();
                ComposerOutcome::Submitted(command)
            }
            CommandReadiness::Incomplete { hint, .. } => ComposerOutcome::Blocked { hint },
            CommandReadiness::Invalid { diagnostic } => {
                ComposerOutcome::Blocked { hint: diagnostic }
            }
        }
    }

    pub fn select_next_or_history_next(&mut self) -> ComposerOutcome {
        if self.suggestions_visible && self.visible_suggestion_count() > 0 {
            let count = self.visible_suggestion_count();
            self.selected_suggestion = Some(
                self.selected_suggestion
                    .map(|current| (current + 1) % count)
                    .unwrap_or(0),
            );
            return ComposerOutcome::Edited;
        }
        self.history_next()
    }

    pub fn select_previous_or_history_previous(&mut self) -> ComposerOutcome {
        if self.suggestions_visible && self.visible_suggestion_count() > 0 {
            let count = self.visible_suggestion_count();
            self.selected_suggestion = Some(
                self.selected_suggestion
                    .map(|current| (current + count - 1) % count)
                    .unwrap_or(count - 1),
            );
            return ComposerOutcome::Edited;
        }
        self.history_previous()
    }

    pub fn escape(&mut self) -> ComposerOutcome {
        if self.suggestions_visible {
            self.suggestions_visible = false;
            self.selected_suggestion = None;
            ComposerOutcome::Edited
        } else {
            ComposerOutcome::Noop
        }
    }

    pub fn ctrl_c(&mut self) -> ComposerOutcome {
        if self.buffer.is_empty() {
            ComposerOutcome::ExitRequested
        } else {
            self.buffer.clear();
            self.cursor = 0;
            self.reset_transient_state();
            ComposerOutcome::Cleared
        }
    }

    pub fn ctrl_d(&mut self) -> ComposerOutcome {
        if self.buffer.is_empty() {
            ComposerOutcome::ExitRequested
        } else {
            ComposerOutcome::Noop
        }
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn push_history(&mut self, command: String) {
        let command = command.trim();
        if command.is_empty() || looks_secret(command) {
            return;
        }
        if self.history.last().is_some_and(|last| last == command) {
            return;
        }
        self.history.push(command.to_string());
    }

    fn apply_selected_if_visible(&mut self) -> bool {
        if !self.suggestions_visible {
            return false;
        }
        let Some(index) = self.selected_suggestion else {
            return false;
        };
        let Some(set) = self.completion_set.clone() else {
            return false;
        };
        let Some(candidate) = set.candidates.get(index) else {
            return false;
        };
        self.apply_candidate(candidate, set.replacement_start, set.replacement_end);
        true
    }

    fn apply_longest_common_prefix(&mut self, set: &CompletionSet) -> bool {
        let Some(prefix) = &set.longest_common_prefix else {
            return false;
        };
        let current = self
            .buffer
            .get(set.replacement_start..set.replacement_end)
            .unwrap_or_default();
        if prefix.len() <= current.len() || !prefix.starts_with(current) {
            return false;
        }
        self.replace_span(set.replacement_start, set.replacement_end, prefix);
        true
    }

    fn apply_candidate(&mut self, candidate: &CompletionCandidate, start: usize, end: usize) {
        self.replace_span(start, end, &candidate.replacement);
    }

    fn replace_span(&mut self, start: usize, end: usize, replacement: &str) {
        let start = clamp_to_boundary(&self.buffer, start);
        let end = clamp_to_boundary(&self.buffer, end);
        self.buffer.replace_range(start..end, replacement);
        self.cursor = start + replacement.len();
        self.reset_completion_state();
    }

    fn history_previous(&mut self) -> ComposerOutcome {
        if self.history.is_empty() {
            return ComposerOutcome::Noop;
        }
        let next_index = match self.history_cursor {
            Some(0) => 0,
            Some(index) => index - 1,
            None => {
                self.history_draft = self.buffer.clone();
                self.history.len() - 1
            }
        };
        self.history_cursor = Some(next_index);
        self.buffer = self.history[next_index].clone();
        self.cursor = self.buffer.len();
        self.reset_completion_state();
        ComposerOutcome::Edited
    }

    fn history_next(&mut self) -> ComposerOutcome {
        let Some(index) = self.history_cursor else {
            return ComposerOutcome::Noop;
        };
        if index + 1 < self.history.len() {
            let next_index = index + 1;
            self.history_cursor = Some(next_index);
            self.buffer = self.history[next_index].clone();
        } else {
            self.history_cursor = None;
            self.buffer = self.history_draft.clone();
            self.history_draft.clear();
        }
        self.cursor = self.buffer.len();
        self.reset_completion_state();
        ComposerOutcome::Edited
    }

    fn leave_history(&mut self) {
        if self.history_cursor.is_some() {
            self.history_cursor = None;
            self.history_draft.clear();
        }
    }

    fn visible_suggestion_count(&self) -> usize {
        self.completion_set
            .as_ref()
            .map(|set| cmp::min(set.candidates.len(), self.max_suggestions))
            .unwrap_or(0)
    }

    fn reset_completion_state(&mut self) {
        self.completion_set = None;
        self.suggestions_visible = false;
        self.selected_suggestion = None;
    }

    fn reset_transient_state(&mut self) {
        self.reset_completion_state();
        self.history_cursor = None;
        self.history_draft.clear();
    }
}

pub fn looks_secret(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let secret_markers = [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "apikey",
        "access_key",
        "private_key",
        "authorization:",
        "bearer ",
    ];
    secret_markers.iter().any(|marker| lower.contains(marker)) || command.contains("sk-")
}

fn format_candidate_hint(candidate: &CompletionCandidate) -> String {
    if candidate.detail.is_empty() {
        candidate.display.clone()
    } else {
        format!("{}: {}", candidate.display, candidate.detail)
    }
}

fn previous_boundary(text: &str, cursor: usize) -> Option<usize> {
    text[..cursor].char_indices().last().map(|(idx, _)| idx)
}

fn next_boundary(text: &str, cursor: usize) -> Option<usize> {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(idx, _)| cursor + idx)
        .or_else(|| (cursor < text.len()).then_some(text.len()))
}

fn clamp_to_boundary(text: &str, mut cursor: usize) -> usize {
    cursor = cmp::min(cursor, text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn previous_word_start(text: &str, cursor: usize) -> Option<usize> {
    if cursor == 0 {
        return None;
    }
    let mut boundary = cursor;
    while let Some(prev) = previous_boundary(text, boundary) {
        let ch = text[prev..boundary].chars().next()?;
        if !ch.is_whitespace() {
            break;
        }
        boundary = prev;
    }
    while let Some(prev) = previous_boundary(text, boundary) {
        let ch = text[prev..boundary].chars().next()?;
        if ch.is_whitespace() {
            break;
        }
        boundary = prev;
    }
    (boundary < cursor).then_some(boundary)
}

fn split_shell_words(text: &str) -> Vec<(usize, usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    for (idx, ch) in text.char_indices() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            start.get_or_insert(idx);
            continue;
        }
        if ch.is_whitespace() {
            if let Some(word_start) = start.take() {
                words.push((word_start, idx, &text[word_start..idx]));
            }
        } else {
            start.get_or_insert(idx);
        }
    }
    if let Some(word_start) = start {
        words.push((word_start, text.len(), &text[word_start..]));
    }
    words
}

fn previous_nonspace_is_value_context(text: &str, start: usize) -> bool {
    let before = text[..start].trim_end();
    before.ends_with('=')
        || before
            .split_whitespace()
            .last()
            .is_some_and(|word| word.starts_with("--"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::{
        CompletionContext, CompletionEngine, CompletionStateView, TaskCompletionItem,
        TaskCompletionScope, TicketCompletionItem, TicketCompletionScope,
    };
    use crate::domain::{RunId, TaskId, TaskStatus, TicketId, TicketStatus};
    use crate::runtime::build_cli;

    const TASK_READY: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TASK_STUCK: &str = "task_01BRZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_OPEN: &str = "ticket_01CRZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01DRZ3NDEKTSV4RRFFQ69G5FAV";

    #[derive(Default)]
    struct FakeStateView;

    impl CompletionStateView for FakeStateView {
        fn tasks_for_completion(
            &self,
            _scope: TaskCompletionScope,
        ) -> HarnessResult<Vec<TaskCompletionItem>> {
            Ok(vec![
                TaskCompletionItem {
                    id: TaskId::parse(TASK_READY).unwrap(),
                    status: TaskStatus::Ready,
                    title: "Fix parser".to_string(),
                },
                TaskCompletionItem {
                    id: TaskId::parse(TASK_STUCK).unwrap(),
                    status: TaskStatus::Stuck,
                    title: "Resolve failure".to_string(),
                },
            ])
        }

        fn tickets_for_completion(
            &self,
            _scope: TicketCompletionScope,
        ) -> HarnessResult<Vec<TicketCompletionItem>> {
            Ok(vec![TicketCompletionItem {
                id: TicketId::parse(TICKET_OPEN).unwrap(),
                task_id: TaskId::parse(TASK_STUCK).unwrap(),
                run_id: RunId::parse(RUN_ID).unwrap(),
                status: TicketStatus::Open,
                summary: "Need decision".to_string(),
            }])
        }
    }

    fn refresh(composer: &mut ComposerState) {
        let catalog = build_cli();
        let state = FakeStateView;
        let engine = CompletionEngine::with_cache_capacity(0);
        composer
            .refresh_completion(
                &engine,
                &CompletionContext {
                    state: &state,
                    repo: None,
                    catalog: &catalog,
                },
            )
            .unwrap();
    }

    #[test]
    fn composer_edits_buffer_and_cursor() {
        let mut composer = ComposerState::new();

        composer.insert_char('a');
        composer.insert_char('β');
        assert_eq!(composer.text(), "aβ");
        assert_eq!(composer.cursor(), "aβ".len());

        composer.move_left();
        assert_eq!(composer.cursor(), 1);
        composer.insert_char('!');
        assert_eq!(composer.text(), "a!β");
        composer.move_right();
        composer.backspace();
        assert_eq!(composer.text(), "a!");
    }

    #[test]
    fn composer_ctrl_a_e_u_w_behaviors() {
        let mut composer = ComposerState::new();
        composer.set_text("task create --title Fix");

        composer.move_to_start();
        assert_eq!(composer.cursor(), 0);
        composer.move_to_end();
        assert_eq!(composer.cursor(), composer.text().len());
        composer.delete_previous_word();
        assert_eq!(composer.text(), "task create --title ");
        composer.clear_before_cursor();
        assert_eq!(composer.text(), "");
    }

    #[test]
    fn composer_suggestion_insertion_uses_replacement_span() {
        let mut composer = ComposerState::new();
        composer.set_text("task get task_zzz");
        composer.move_to_start();
        for _ in 0.."task get task".len() {
            composer.move_right();
        }
        refresh(&mut composer);

        assert!(
            composer
                .suggestion_rows()
                .iter()
                .any(|row| row.display.contains("Fix parser"))
        );
        composer.select_next_or_history_next();
        assert_eq!(composer.tab_complete(), ComposerOutcome::AppliedSuggestion);
        assert_eq!(composer.text(), format!("task get {TASK_READY}"));
    }

    #[test]
    fn composer_scoped_id_completion_keeps_display_context_out_of_inserted_text() {
        let mut composer = ComposerState::new();
        composer.set_text(format!("resume {TASK_STUCK} --ticket ticket_"));
        refresh(&mut composer);

        let rows = composer.suggestion_rows();
        assert!(rows.iter().any(|row| row.display.contains("Need decision")));
        assert!(rows.iter().any(|row| row.replacement == TICKET_OPEN));

        composer.select_next_or_history_next();
        assert_eq!(composer.tab_complete(), ComposerOutcome::AppliedSuggestion);
        assert_eq!(
            composer.text(),
            format!("resume {TASK_STUCK} --ticket {TICKET_OPEN}")
        );
    }

    #[test]
    fn composer_tab_uses_longest_common_prefix_before_menu_selection() {
        let mut composer = ComposerState::new();
        composer.set_text("t");
        composer.set_completion_set(CompletionSet {
            replacement_start: 0,
            replacement_end: 1,
            candidates: vec![
                CompletionCandidate {
                    replacement: "ticket".to_string(),
                    display: "ticket".to_string(),
                    detail: String::new(),
                    kind: CompletionKind::Command,
                },
                CompletionCandidate {
                    replacement: "title".to_string(),
                    display: "title".to_string(),
                    detail: String::new(),
                    kind: CompletionKind::Command,
                },
            ],
            longest_common_prefix: Some("ti".to_string()),
            status: CompletionStatus::Ready,
            hint: None,
            readiness: CommandReadiness::Incomplete {
                missing: vec!["command".to_string()],
                hint: "Choose a command.".to_string(),
            },
        });

        assert_eq!(composer.tab_complete(), ComposerOutcome::AppliedSuggestion);
        assert_eq!(composer.text(), "ti");

        composer.set_text("tas");
        refresh(&mut composer);
        assert_eq!(composer.tab_complete(), ComposerOutcome::AppliedSuggestion);
        assert_eq!(composer.text(), "task");
    }

    #[test]
    fn composer_enter_applies_visible_selected_suggestion_before_submit() {
        let mut composer = ComposerState::new();
        composer.set_text("task r");
        refresh(&mut composer);

        assert!(composer.suggestions_visible());
        composer.select_next_or_history_next();
        assert_eq!(composer.enter(), ComposerOutcome::AppliedSuggestion);
        assert_eq!(composer.text(), "task run");
    }

    #[test]
    fn composer_enter_submits_ready_commands_and_blocks_incomplete_commands() {
        let mut composer = ComposerState::new();
        composer.set_text("task get");
        refresh(&mut composer);
        assert!(matches!(composer.enter(), ComposerOutcome::Blocked { .. }));

        composer.set_text(format!("task get {TASK_READY}"));
        refresh(&mut composer);
        assert_eq!(
            composer.enter(),
            ComposerOutcome::Submitted(format!("task get {TASK_READY}"))
        );
    }

    #[test]
    fn composer_shell_escape_hint_is_exposed_without_harness_suggestions() {
        let mut composer = ComposerState::new();
        composer.set_text("!cargo test");
        refresh(&mut composer);

        assert!(composer.suggestion_rows().is_empty());
        assert!(
            composer
                .shell_escape_hint()
                .unwrap()
                .contains("repository root")
        );
    }

    #[test]
    fn composer_history_navigation_and_secret_filtering() {
        let mut composer = ComposerState::new();
        composer.push_history("task list".to_string());
        composer.push_history("OPENAI_API_KEY=sk-test-secret task run".to_string());
        composer.push_history("ticket list".to_string());

        assert_eq!(composer.history(), &["task list", "ticket list"]);
        assert_eq!(
            composer.select_previous_or_history_previous(),
            ComposerOutcome::Edited
        );
        assert_eq!(composer.text(), "ticket list");
        composer.select_previous_or_history_previous();
        assert_eq!(composer.text(), "task list");
        composer.select_next_or_history_next();
        assert_eq!(composer.text(), "ticket list");
        composer.select_next_or_history_next();
        assert_eq!(composer.text(), "");
    }

    #[test]
    fn composer_up_down_select_suggestions_before_history() {
        let mut composer = ComposerState::new();
        composer.set_text("task ");
        refresh(&mut composer);

        let first = composer.selected_suggestion();
        composer.select_next_or_history_next();
        assert_ne!(composer.selected_suggestion(), first);
        composer.select_previous_or_history_previous();
        assert_eq!(
            composer.selected_suggestion(),
            Some(composer.suggestion_rows().len() - 1)
        );
    }

    #[test]
    fn composer_syntax_tokens_cover_command_option_value_and_error() {
        let mut composer = ComposerState::new();
        composer.set_text("task list --status ready");
        refresh(&mut composer);
        let classes = composer
            .syntax_tokens()
            .into_iter()
            .map(|token| token.class)
            .collect::<Vec<_>>();
        assert!(classes.contains(&SyntaxClass::Command));
        assert!(classes.contains(&SyntaxClass::Option));
        assert!(classes.contains(&SyntaxClass::Value));

        composer.set_text("not-a-command ");
        refresh(&mut composer);
        assert_eq!(composer.syntax_tokens()[0].class, SyntaxClass::Error);
    }

    #[test]
    fn composer_ctrl_c_d_and_escape_behaviors() {
        let mut composer = ComposerState::new();
        composer.set_text("task ");
        refresh(&mut composer);
        assert_eq!(composer.escape(), ComposerOutcome::Edited);
        assert!(!composer.suggestions_visible());

        assert_eq!(composer.ctrl_c(), ComposerOutcome::Cleared);
        assert_eq!(composer.ctrl_c(), ComposerOutcome::ExitRequested);
        assert_eq!(composer.ctrl_d(), ComposerOutcome::ExitRequested);
    }

    #[test]
    fn composer_ctrl_d_exits_only_when_empty() {
        let mut composer = ComposerState::new();
        composer.set_text("task list");

        assert_eq!(composer.ctrl_d(), ComposerOutcome::Noop);
        assert_eq!(composer.text(), "task list");

        composer.set_text("");
        assert_eq!(composer.ctrl_d(), ComposerOutcome::ExitRequested);
    }
}
