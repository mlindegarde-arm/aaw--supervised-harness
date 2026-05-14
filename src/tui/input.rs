use crate::tui::composer::{ComposerOutcome, ComposerState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    bits: u8,
}

impl KeyModifiers {
    pub const CONTROL: Self = Self { bits: 0b0000_0001 };

    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn contains(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyEvent {
    pub const fn new(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::empty(),
        }
    }

    pub const fn ctrl(ch: char) -> Self {
        Self {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::CONTROL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerCommand {
    Insert(char),
    Complete,
    SubmitOrApplySuggestion,
    SelectPreviousOrHistoryPrevious,
    SelectNextOrHistoryNext,
    MoveLeft,
    MoveRight,
    Backspace,
    MoveToStart,
    MoveToEnd,
    ClearBeforeCursor,
    DeletePreviousWord,
    PreviousPane,
    NextPane,
    ScrollPageUp,
    ScrollPageDown,
    Interrupt,
    EndOfInput,
    Escape,
    Ignore,
}

pub fn command_for_key(event: KeyEvent) -> ComposerCommand {
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        return match event.code {
            KeyCode::Char('a') | KeyCode::Char('A') => ComposerCommand::MoveToStart,
            KeyCode::Char('e') | KeyCode::Char('E') => ComposerCommand::MoveToEnd,
            KeyCode::Char('u') | KeyCode::Char('U') => ComposerCommand::ClearBeforeCursor,
            KeyCode::Char('w') | KeyCode::Char('W') => ComposerCommand::DeletePreviousWord,
            KeyCode::Char('p') | KeyCode::Char('P') => ComposerCommand::PreviousPane,
            KeyCode::Char('n') | KeyCode::Char('N') => ComposerCommand::NextPane,
            KeyCode::Char('c') | KeyCode::Char('C') => ComposerCommand::Interrupt,
            KeyCode::Char('d') | KeyCode::Char('D') => ComposerCommand::EndOfInput,
            _ => ComposerCommand::Ignore,
        };
    }

    match event.code {
        KeyCode::Char(ch) => ComposerCommand::Insert(ch),
        KeyCode::Enter => ComposerCommand::SubmitOrApplySuggestion,
        KeyCode::Tab => ComposerCommand::Complete,
        KeyCode::Backspace => ComposerCommand::Backspace,
        KeyCode::Left => ComposerCommand::MoveLeft,
        KeyCode::Right => ComposerCommand::MoveRight,
        KeyCode::Home => ComposerCommand::MoveToStart,
        KeyCode::End => ComposerCommand::MoveToEnd,
        KeyCode::PageUp => ComposerCommand::ScrollPageUp,
        KeyCode::PageDown => ComposerCommand::ScrollPageDown,
        KeyCode::Up => ComposerCommand::SelectPreviousOrHistoryPrevious,
        KeyCode::Down => ComposerCommand::SelectNextOrHistoryNext,
        KeyCode::Esc => ComposerCommand::Escape,
    }
}

pub fn apply_key(composer: &mut ComposerState, event: KeyEvent) -> ComposerOutcome {
    apply_command(composer, command_for_key(event))
}

pub fn apply_command(composer: &mut ComposerState, command: ComposerCommand) -> ComposerOutcome {
    match command {
        ComposerCommand::Insert(ch) => composer.insert_char(ch),
        ComposerCommand::Complete => composer.tab_complete(),
        ComposerCommand::SubmitOrApplySuggestion => composer.enter(),
        ComposerCommand::SelectPreviousOrHistoryPrevious => {
            composer.select_previous_or_history_previous()
        }
        ComposerCommand::SelectNextOrHistoryNext => composer.select_next_or_history_next(),
        ComposerCommand::MoveLeft => composer.move_left(),
        ComposerCommand::MoveRight => composer.move_right(),
        ComposerCommand::Backspace => composer.backspace(),
        ComposerCommand::MoveToStart => composer.move_to_start(),
        ComposerCommand::MoveToEnd => composer.move_to_end(),
        ComposerCommand::ClearBeforeCursor => composer.clear_before_cursor(),
        ComposerCommand::DeletePreviousWord => composer.delete_previous_word(),
        ComposerCommand::PreviousPane
        | ComposerCommand::NextPane
        | ComposerCommand::ScrollPageUp
        | ComposerCommand::ScrollPageDown => ComposerOutcome::Noop,
        ComposerCommand::Interrupt => composer.ctrl_c(),
        ComposerCommand::EndOfInput => composer.ctrl_d(),
        ComposerCommand::Escape => composer.escape(),
        ComposerCommand::Ignore => ComposerOutcome::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_maps_documented_keys_to_composer_commands() {
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('x'))),
            ComposerCommand::Insert('x')
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Enter)),
            ComposerCommand::SubmitOrApplySuggestion
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Tab)),
            ComposerCommand::Complete
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Backspace)),
            ComposerCommand::Backspace
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Left)),
            ComposerCommand::MoveLeft
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Right)),
            ComposerCommand::MoveRight
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Home)),
            ComposerCommand::MoveToStart
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::End)),
            ComposerCommand::MoveToEnd
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::PageUp)),
            ComposerCommand::ScrollPageUp
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::PageDown)),
            ComposerCommand::ScrollPageDown
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Up)),
            ComposerCommand::SelectPreviousOrHistoryPrevious
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Down)),
            ComposerCommand::SelectNextOrHistoryNext
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Esc)),
            ComposerCommand::Escape
        );
    }

    #[test]
    fn input_maps_control_bindings_to_composer_commands() {
        assert_eq!(
            command_for_key(KeyEvent::ctrl('a')),
            ComposerCommand::MoveToStart
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('e')),
            ComposerCommand::MoveToEnd
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('u')),
            ComposerCommand::ClearBeforeCursor
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('w')),
            ComposerCommand::DeletePreviousWord
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('p')),
            ComposerCommand::PreviousPane
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('n')),
            ComposerCommand::NextPane
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('c')),
            ComposerCommand::Interrupt
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('d')),
            ComposerCommand::EndOfInput
        );
        assert_eq!(
            command_for_key(KeyEvent::ctrl('z')),
            ComposerCommand::Ignore
        );
    }

    #[test]
    fn input_apply_key_edits_composer() {
        let mut composer = ComposerState::new();

        assert_eq!(
            apply_key(&mut composer, KeyEvent::new(KeyCode::Char('h'))),
            ComposerOutcome::Edited
        );
        apply_key(&mut composer, KeyEvent::new(KeyCode::Char('i')));
        assert_eq!(composer.text(), "hi");
        apply_key(&mut composer, KeyEvent::new(KeyCode::Left));
        apply_key(&mut composer, KeyEvent::new(KeyCode::Backspace));
        assert_eq!(composer.text(), "i");
        apply_key(&mut composer, KeyEvent::new(KeyCode::Right));
        apply_key(&mut composer, KeyEvent::ctrl('u'));
        assert_eq!(composer.text(), "");
    }

    #[test]
    fn input_home_and_end_match_ctrl_a_and_ctrl_e() {
        let mut composer = ComposerState::new();
        composer.set_text("task list");

        assert_eq!(
            apply_key(&mut composer, KeyEvent::new(KeyCode::Home)),
            ComposerOutcome::Edited
        );
        assert_eq!(composer.cursor(), 0);
        assert_eq!(
            apply_key(&mut composer, KeyEvent::new(KeyCode::End)),
            ComposerOutcome::Edited
        );
        assert_eq!(composer.cursor(), composer.text().len());

        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Home)),
            command_for_key(KeyEvent::ctrl('a'))
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::End)),
            command_for_key(KeyEvent::ctrl('e'))
        );
    }

    #[test]
    fn input_ctrl_d_exits_only_when_composer_is_empty() {
        let mut composer = ComposerState::new();
        composer.set_text("task list");

        assert_eq!(
            apply_key(&mut composer, KeyEvent::ctrl('d')),
            ComposerOutcome::Noop
        );
        assert_eq!(composer.text(), "task list");

        composer.set_text("");
        assert_eq!(
            apply_key(&mut composer, KeyEvent::ctrl('d')),
            ComposerOutcome::ExitRequested
        );
    }
}
