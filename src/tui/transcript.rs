use crate::runtime::{CommandEvent, CommandExit, SuperviseProgressEvent, TranscriptEvent};
use crate::security::{DefaultRedactor, Redactor};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptSource {
    Stdout,
    Stderr,
    Command,
    Progress,
    Error,
    System,
}

impl TranscriptSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stdout => "out",
            Self::Stderr => "err",
            Self::Command => "cmd",
            Self::Progress => "run",
            Self::Error => "error",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub source: TranscriptSource,
    pub text: String,
    pub truncated: bool,
    pub secret_redacted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptLimits {
    pub max_entries: usize,
    pub max_entry_lines: usize,
    pub max_entry_bytes: usize,
}

impl Default for TranscriptLimits {
    fn default() -> Self {
        Self {
            max_entries: 2_000,
            max_entry_lines: 200,
            max_entry_bytes: 16 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TranscriptState {
    entries: VecDeque<TranscriptEntry>,
    limits: TranscriptLimits,
    scroll_from_bottom: u16,
}

impl Default for TranscriptState {
    fn default() -> Self {
        Self::new(TranscriptLimits::default())
    }
}

impl TranscriptState {
    pub fn new(limits: TranscriptLimits) -> Self {
        assert!(limits.max_entries > 0, "transcript must retain entries");
        assert!(limits.max_entry_lines > 0, "transcript must retain lines");
        assert!(limits.max_entry_bytes > 0, "transcript must retain bytes");
        Self {
            entries: VecDeque::new(),
            limits,
            scroll_from_bottom: 0,
        }
    }

    pub fn entries(&self) -> impl ExactSizeIterator<Item = &TranscriptEntry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn scroll_from_bottom(&self) -> u16 {
        self.scroll_from_bottom
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }

    pub fn append_event(&mut self, event: TranscriptEvent) {
        match event {
            TranscriptEvent::Stdout(text) => {
                self.append_untrusted_text(TranscriptSource::Stdout, &text);
            }
            TranscriptEvent::Stderr(text) => {
                self.append_untrusted_text(TranscriptSource::Stderr, &text);
            }
            TranscriptEvent::Command(event) => self.append_command_event(&event),
            TranscriptEvent::SuperviseProgress(event) => self.append_progress_event(&event),
            TranscriptEvent::CommandFinished(exit) => self.append_command_finished(&exit),
            TranscriptEvent::CancellationAcknowledged { next_command } => {
                let text = next_command.map_or_else(
                    || "cancellation acknowledged".to_string(),
                    |cmd| format!("cancellation acknowledged; resume with `{cmd}`"),
                );
                self.append_untrusted_text(TranscriptSource::System, &text);
            }
            TranscriptEvent::Error(text) => {
                self.append_untrusted_text(TranscriptSource::Error, &text);
            }
        }
    }

    pub fn append_untrusted_text(&mut self, source: TranscriptSource, text: &str) {
        let sanitized_text = sanitize_untrusted_text(text);
        let sanitized = sanitized_text.text;
        let (text, truncated) = apply_caps(&sanitized, self.limits);

        self.entries.push_back(TranscriptEntry {
            source,
            text,
            truncated,
            secret_redacted: sanitized_text.secret_redacted,
        });

        while self.entries.len() > self.limits.max_entries {
            self.entries.pop_front();
        }
        self.scroll_to_bottom();
    }

    fn append_command_event(&mut self, event: &CommandEvent) {
        let text = format!("{}: {}", event.level.as_str(), event.message);
        self.append_untrusted_text(TranscriptSource::Command, &text);
    }

    fn append_progress_event(&mut self, event: &SuperviseProgressEvent) {
        let mut parts = vec![format!("{:?}", event.phase)];
        if let Some(task_id) = &event.task_id {
            parts.push(task_id.to_string());
        }
        if let Some(run_id) = &event.run_id {
            parts.push(run_id.to_string());
        }
        if let Some(ticket_id) = &event.ticket_id {
            parts.push(ticket_id.to_string());
        }
        if let Some(cycle) = event.cycle {
            parts.push(format!("cycle {cycle}"));
        }
        parts.push(event.message.clone());
        if let Some(next_command) = &event.next_command {
            parts.push(format!("next: {next_command}"));
        }
        self.append_untrusted_text(TranscriptSource::Progress, &parts.join(" | "));
    }

    fn append_command_finished(&mut self, exit: &CommandExit) {
        let message = exit
            .message
            .as_deref()
            .map_or_else(String::new, |message| format!(": {message}"));
        let text = format!(
            "command finished: {} ({}){}",
            exit.status.as_str(),
            exit.exit_code,
            message
        );
        self.append_untrusted_text(TranscriptSource::System, &text);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SanitizedText {
    pub text: String,
    pub secret_redacted: bool,
}

pub(crate) fn sanitize_untrusted_text(text: &str) -> SanitizedText {
    let redacted = DefaultRedactor::new().redact(text);
    let (text, inline_secret_redacted) = redact_inline_sensitive_assignments(&redacted.text);
    SanitizedText {
        text: sanitize_terminal_controls(&text),
        secret_redacted: redacted.high_confidence_secret_detected || inline_secret_redacted,
    }
}

fn apply_caps(input: &str, limits: TranscriptLimits) -> (String, bool) {
    let mut truncated = false;
    let mut output = String::new();

    for (idx, line) in input.lines().enumerate() {
        if idx >= limits.max_entry_lines {
            truncated = true;
            break;
        }
        if idx > 0 {
            push_capped(&mut output, "\n", limits.max_entry_bytes, &mut truncated);
        }
        push_capped(&mut output, line, limits.max_entry_bytes, &mut truncated);
        if truncated {
            break;
        }
    }

    if input.ends_with('\n') && !truncated && input.lines().count() < limits.max_entry_lines {
        push_capped(&mut output, "\n", limits.max_entry_bytes, &mut truncated);
    }

    if truncated {
        append_marker(&mut output, "\n[truncated]");
    }

    (output, truncated)
}

fn push_capped(output: &mut String, chunk: &str, max_bytes: usize, truncated: &mut bool) {
    if *truncated || output.len() >= max_bytes {
        *truncated = true;
        return;
    }

    for ch in chunk.chars() {
        let next_len = output.len() + ch.len_utf8();
        if next_len > max_bytes {
            *truncated = true;
            return;
        }
        output.push(ch);
    }
}

fn append_marker(output: &mut String, marker: &str) {
    output.push_str(marker);
}

fn redact_inline_sensitive_assignments(input: &str) -> (String, bool) {
    let mut output = String::with_capacity(input.len());
    let mut token = String::new();
    let mut redacted = false;

    for ch in input.chars() {
        if ch.is_whitespace() {
            push_redacted_token(&mut output, &token, &mut redacted);
            token.clear();
            output.push(ch);
        } else {
            token.push(ch);
        }
    }
    push_redacted_token(&mut output, &token, &mut redacted);

    (output, redacted)
}

fn push_redacted_token(output: &mut String, token: &str, redacted: &mut bool) {
    if token.is_empty() {
        return;
    }

    let Some((key, _value)) = token.split_once('=') else {
        output.push_str(token);
        return;
    };

    let key_start = key
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .map_or(0, |idx| idx + 1);
    let (prefix, key_name) = key.split_at(key_start);
    if is_sensitive_assignment_key(key_name) {
        *redacted = true;
        output.push_str(prefix);
        output.push_str(key_name);
        output.push_str("=[REDACTED]");
    } else {
        output.push_str(token);
    }
}

fn is_sensitive_assignment_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("API_KEY")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("PASSWD")
        || upper.contains("SECRET")
        || upper.contains("COOKIE")
        || upper.contains("PRIVATE_KEY")
        || upper == "AUTHORIZATION"
        || upper == "PROXY_AUTHORIZATION"
        || upper == "SSH_AUTH_SOCK"
}

pub(crate) fn sanitize_terminal_controls(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => strip_escape_sequence(&mut chars),
            '\n' => output.push('\n'),
            '\r' => output.push('\n'),
            '\t' => output.push('\t'),
            ch if ch.is_control() => {}
            ch => output.push(ch),
        }
    }

    output
}

fn strip_escape_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    match chars.next() {
        Some('[') => strip_until_csi_final(chars),
        Some(']') => strip_until_string_terminator(chars),
        Some('P' | '^' | '_' | 'X') => strip_until_string_terminator(chars),
        Some(_) | None => {}
    }
}

fn strip_until_csi_final<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    for ch in chars.by_ref() {
        if ('\x40'..='\x7e').contains(&ch) {
            break;
        }
    }
}

fn strip_until_string_terminator<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    while let Some(ch) = chars.next() {
        if ch == '\x07' {
            break;
        }
        if ch == '\x1b' && matches!(chars.peek(), Some('\\')) {
            chars.next();
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_redacts_secret_assignments() {
        let mut transcript = TranscriptState::default();
        transcript.append_untrusted_text(TranscriptSource::Stdout, "OPENAI_API_KEY=sk-test-secret");

        let entry = transcript.entries().next().unwrap();
        assert!(entry.secret_redacted);
        assert!(entry.text.contains("[REDACTED"));
        assert!(!entry.text.contains("sk-test-secret"));
    }

    #[test]
    fn transcript_strips_ansi_osc52_and_clear_screen_controls() {
        let mut transcript = TranscriptState::default();
        transcript.append_untrusted_text(
            TranscriptSource::Stdout,
            "ok\x1b[31m red\x1b[0m\x1b]52;c;YWJj\x07\x1b[2Jdone\x08",
        );

        let text = &transcript.entries().next().unwrap().text;
        assert_eq!(text, "ok reddone");
        assert!(!text.contains('\x1b'));
        assert!(!text.contains('\x08'));
    }

    #[test]
    fn transcript_caps_lines_and_bytes_with_marker() {
        let limits = TranscriptLimits {
            max_entries: 3,
            max_entry_lines: 2,
            max_entry_bytes: 12,
        };
        let mut transcript = TranscriptState::new(limits);
        transcript.append_untrusted_text(TranscriptSource::Stdout, "first\nsecond\nthird");

        let entry = transcript.entries().next().unwrap();
        assert!(entry.truncated);
        assert_eq!(entry.text, "first\nsecond\n[truncated]");
    }

    #[test]
    fn transcript_scrolls_and_prunes_entries() {
        let limits = TranscriptLimits {
            max_entries: 2,
            max_entry_lines: 10,
            max_entry_bytes: 100,
        };
        let mut transcript = TranscriptState::new(limits);
        transcript.append_untrusted_text(TranscriptSource::System, "one");
        transcript.scroll_up(5);
        assert_eq!(transcript.scroll_from_bottom(), 5);

        transcript.append_untrusted_text(TranscriptSource::System, "two");
        transcript.append_untrusted_text(TranscriptSource::System, "three");

        assert_eq!(transcript.scroll_from_bottom(), 0);
        let entries = transcript
            .entries()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec!["two", "three"]);
    }
}
