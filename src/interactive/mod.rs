use crate::error::{HarnessError, HarnessResult};
use crate::runtime::{CommandExit, CommandRuntime, InteractiveSink};
use crate::security::{DefaultRedactor, Redactor};
use crate::workspace::{CommandRunner, CommandSpec, CommandStdin, GitWorkspaceManager};
use rustyline::{DefaultEditor, error::ReadlineError};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};

const PROMPT: &str = "harness> ";
const SHELL_PATH: &str = "/bin/sh";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineRead {
    Line(String),
    Eof,
    Interrupted,
}

pub trait LineEditor {
    fn read_line(&mut self, prompt: &str, stdout: &mut dyn Write) -> HarnessResult<LineRead>;
    fn add_history_entry(&mut self, line: &str);
    fn history(&self) -> &[String];
}

pub struct TerminalLineEditor {
    editor: DefaultEditor,
    history: Vec<String>,
}

impl TerminalLineEditor {
    pub fn new() -> HarnessResult<Self> {
        let editor = DefaultEditor::new().map_err(|err| {
            HarnessError::External(format!("failed to initialize terminal line editor: {err}"))
        })?;

        Ok(Self {
            editor,
            history: Vec::new(),
        })
    }
}

impl LineEditor for TerminalLineEditor {
    fn read_line(&mut self, prompt: &str, _stdout: &mut dyn Write) -> HarnessResult<LineRead> {
        match self.editor.readline(prompt) {
            Ok(line) => Ok(LineRead::Line(line)),
            Err(ReadlineError::Interrupted) => Ok(LineRead::Interrupted),
            Err(ReadlineError::Eof) => Ok(LineRead::Eof),
            Err(err) => Err(HarnessError::External(format!(
                "failed to read command input: {err}"
            ))),
        }
    }

    fn add_history_entry(&mut self, line: &str) {
        let _ = self.editor.add_history_entry(line);
        self.history.push(line.to_string());
    }

    fn history(&self) -> &[String] {
        &self.history
    }
}

pub struct BufReadLineEditor<'a> {
    stdin: &'a mut dyn BufRead,
    history: Vec<String>,
    history_cursor: Option<usize>,
}

impl<'a> BufReadLineEditor<'a> {
    pub fn new(stdin: &'a mut dyn BufRead) -> Self {
        Self {
            stdin,
            history: Vec::new(),
            history_cursor: None,
        }
    }
}

impl LineEditor for BufReadLineEditor<'_> {
    fn read_line(&mut self, prompt: &str, stdout: &mut dyn Write) -> HarnessResult<LineRead> {
        write!(stdout, "{prompt}").map_err(write_error)?;
        stdout.flush().map_err(write_error)?;

        let mut line = String::new();
        match self.stdin.read_line(&mut line) {
            Ok(0) => Ok(LineRead::Eof),
            Ok(_) => Ok(self.line_or_history_recall(line)),
            Err(err) => Err(HarnessError::External(format!(
                "failed to read command input: {err}"
            ))),
        }
    }

    fn add_history_entry(&mut self, line: &str) {
        self.history.push(line.to_string());
        self.history_cursor = None;
    }

    fn history(&self) -> &[String] {
        &self.history
    }
}

impl BufReadLineEditor<'_> {
    fn line_or_history_recall(&mut self, line: String) -> LineRead {
        match line.trim_end_matches(|ch| ch == '\r' || ch == '\n') {
            "\u{1b}[A" => LineRead::Line(self.recall_previous().unwrap_or_default()),
            "\u{1b}[B" => LineRead::Line(self.recall_next().unwrap_or_default()),
            _ => LineRead::Line(line),
        }
    }

    fn recall_previous(&mut self) -> Option<String> {
        if self.history.is_empty() {
            return None;
        }
        let next = self
            .history_cursor
            .unwrap_or(self.history.len())
            .saturating_sub(1);
        self.history_cursor = Some(next);
        self.history.get(next).cloned()
    }

    fn recall_next(&mut self) -> Option<String> {
        let cursor = self.history_cursor?;
        let next = cursor + 1;
        if next >= self.history.len() {
            self.history_cursor = None;
            None
        } else {
            self.history_cursor = Some(next);
            self.history.get(next).cloned()
        }
    }
}

pub struct InteractiveShell<'runtime, 'service, R> {
    runtime: &'runtime CommandRuntime<'service>,
    shell_runner: R,
}

impl<'runtime, 'service> InteractiveShell<'runtime, 'service, GitWorkspaceManager> {
    pub fn with_default_workspace(runtime: &'runtime CommandRuntime<'service>) -> Self {
        Self::new(runtime, GitWorkspaceManager::default())
    }
}

impl<'runtime, 'service, R> InteractiveShell<'runtime, 'service, R>
where
    R: CommandRunner,
{
    pub fn new(runtime: &'runtime CommandRuntime<'service>, shell_runner: R) -> Self {
        Self {
            runtime,
            shell_runner,
        }
    }

    pub fn run(
        &self,
        editor: &mut dyn LineEditor,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) -> CommandExit {
        if let Err(err) = writeln!(
            stdout,
            "harness interactive mode; type exit or quit to leave."
        ) {
            return CommandExit::failure(format!("failed to write command output: {err}"));
        }

        loop {
            let read = match editor.read_line(PROMPT, stdout) {
                Ok(read) => read,
                Err(err) => return CommandExit::failure(err.to_string()),
            };

            let command = match read {
                LineRead::Line(line) => line.trim().to_string(),
                LineRead::Eof => return CommandExit::success(),
                LineRead::Interrupted => {
                    let _ = writeln!(stdout);
                    continue;
                }
            };

            if command.is_empty() {
                continue;
            }
            if command == "exit" || command == "quit" {
                return CommandExit::success();
            }

            if should_record_history(&command) {
                editor.add_history_entry(&command);
            }

            let quiet = command.split_whitespace().any(|token| token == "--quiet");
            let mut sink = InteractiveSink::new(stdout, stderr, quiet);
            if let Some(shell_command) = command.strip_prefix('!') {
                self.run_shell_escape(shell_command.trim_start(), &mut sink);
                continue;
            }

            self.runtime.dispatch_line(&command, &mut sink);
        }
    }

    fn run_shell_escape(&self, command: &str, sink: &mut InteractiveSink<'_>) {
        if command.trim().is_empty() {
            let _ = sink.write_stderr_line("shell escape command cannot be empty");
            return;
        }

        let spec = CommandSpec {
            command: command.to_string(),
            cwd: std::env::current_dir()
                .map(path_to_string)
                .unwrap_or_else(|_| ".".to_string()),
            shell_path: SHELL_PATH.to_string(),
            env: std::env::vars().collect::<BTreeMap<_, _>>(),
            timeout_seconds: 3600,
            max_output_bytes: 1024 * 1024,
            stdin: CommandStdin::Null,
            kill_process_group_on_timeout: true,
        };

        match self.shell_runner.run_shell_escape(spec) {
            Ok(output) => {
                let _ = sink.write_stdout(output.stdout.as_bytes());
                let _ = sink.write_stderr(output.stderr.as_bytes());
                if !output.timed_out && output.exit_code == Some(0) {
                    return;
                }
                if output.timed_out {
                    let _ = sink.write_stderr_line("shell escape timed out");
                } else {
                    let code = output
                        .exit_code
                        .map_or_else(|| "signal".to_string(), |code| code.to_string());
                    let _ = sink.write_stderr_line(&format!("shell escape exited with {code}"));
                }
            }
            Err(err) => {
                let _ = sink.write_stderr_line(&format!("failed to run shell escape: {err}"));
            }
        }
    }
}

pub fn run_with_input(
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    runtime: &CommandRuntime<'_>,
) -> CommandExit {
    let shell = InteractiveShell::with_default_workspace(runtime);

    #[cfg(test)]
    {
        let mut editor = BufReadLineEditor::new(stdin);
        return shell.run(&mut editor, stdout, stderr);
    }

    #[cfg(not(test))]
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let mut editor = match TerminalLineEditor::new() {
            Ok(editor) => editor,
            Err(err) => return CommandExit::failure(err.to_string()),
        };
        shell.run(&mut editor, stdout, stderr)
    } else {
        let mut editor = BufReadLineEditor::new(stdin);
        shell.run(&mut editor, stdout, stderr)
    }
}

fn should_record_history(command: &str) -> bool {
    !DefaultRedactor::new()
        .redact(command)
        .high_confidence_secret_detected
}

fn path_to_string(path: std::path::PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn write_error(err: std::io::Error) -> HarnessError {
    HarnessError::External(format!("failed to write command output: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Task, TaskId, Ticket, TicketId};
    use crate::runtime::{CommandResult, ResumeTaskOptions, TaskRunOptions, TicketResolveOptions};
    use crate::service::HarnessService;
    use crate::workspace::CommandOutput;
    use std::cell::RefCell;

    #[test]
    fn leading_harness_is_optional_in_interactive_input() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::default();
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor =
            VecLineEditor::new([LineRead::Line("harness version\n".into()), LineRead::Eof]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert!(
            String::from_utf8(stdout)
                .unwrap()
                .contains(&format!("harness {}", env!("CARGO_PKG_VERSION")))
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn invalid_commands_keep_the_shell_open() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::default();
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor = VecLineEditor::new([
            LineRead::Line("task nope\n".into()),
            LineRead::Line("version\n".into()),
            LineRead::Line("exit\n".into()),
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert!(
            String::from_utf8(stdout)
                .unwrap()
                .contains(&format!("harness {}", env!("CARGO_PKG_VERSION")))
        );
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("unknown task subcommand")
        );
    }

    #[test]
    fn exit_quit_eof_and_empty_input_behavior() {
        for line in ["exit\n", "quit\n", "\n"] {
            let service = StubService;
            let runtime = CommandRuntime::new(&service);
            let runner = RecordingRunner::default();
            let shell = InteractiveShell::new(&runtime, runner);
            let mut editor = if line.trim().is_empty() {
                VecLineEditor::new([LineRead::Line(line.into()), LineRead::Eof])
            } else {
                VecLineEditor::new([LineRead::Line(line.into())])
            };
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

            assert_eq!(exit.code(), 0);
            assert!(stderr.is_empty());
        }
    }

    #[test]
    fn ctrl_c_interrupt_keeps_the_shell_open() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::default();
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor = VecLineEditor::new([
            LineRead::Interrupted,
            LineRead::Line("version\n".into()),
            LineRead::Eof,
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert!(
            String::from_utf8(stdout)
                .unwrap()
                .contains(&format!("harness {}", env!("CARGO_PKG_VERSION")))
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn shell_escape_dispatches_without_runtime_and_keeps_shell_open() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::with_output("shell-ok\n", "", Some(0));
        let records = runner.records.clone();
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor = VecLineEditor::new([
            LineRead::Line("! printf shell-ok\n".into()),
            LineRead::Line("version\n".into()),
            LineRead::Eof,
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.contains("shell-ok"));
        assert!(stdout.contains(&format!("harness {}", env!("CARGO_PKG_VERSION"))));
        assert_eq!(records.borrow().as_slice(), ["printf shell-ok".to_string()]);
        assert!(stderr.is_empty());
    }

    #[test]
    fn shell_escape_writes_output_and_status_through_interactive_sink() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::with_output("shell-out\n", "shell-err\n", Some(7));
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor = VecLineEditor::new([
            LineRead::Line("! failing-command\n".into()),
            LineRead::Line("version\n".into()),
            LineRead::Eof,
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stdout.contains("shell-out"));
        assert!(stdout.contains(&format!("harness {}", env!("CARGO_PKG_VERSION"))));
        assert!(stderr.contains("shell-err"));
        assert!(stderr.contains("shell escape exited with 7"));
    }

    #[test]
    fn empty_shell_escape_reports_usage_and_keeps_shell_open() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::default();
        let records = runner.records.clone();
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor = VecLineEditor::new([
            LineRead::Line("!\n".into()),
            LineRead::Line("version\n".into()),
            LineRead::Eof,
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert!(
            String::from_utf8(stdout)
                .unwrap()
                .contains(&format!("harness {}", env!("CARGO_PKG_VERSION")))
        );
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("shell escape command cannot be empty")
        );
        assert!(records.borrow().is_empty());
    }

    #[test]
    fn secret_looking_commands_are_not_recorded_in_history() {
        let service = StubService;
        let runtime = CommandRuntime::new(&service);
        let runner = RecordingRunner::default();
        let shell = InteractiveShell::new(&runtime, runner);
        let mut editor = VecLineEditor::new([
            LineRead::Line("API_KEY=sk-test-secret version\n".into()),
            LineRead::Line("version\n".into()),
            LineRead::Eof,
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = shell.run(&mut editor, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert_eq!(editor.history(), ["version"]);
    }

    #[test]
    fn terminal_line_editor_records_history_for_rustyline_recall() {
        let mut editor = TerminalLineEditor::new().unwrap();

        editor.add_history_entry("version");
        editor.add_history_entry("task list");

        assert_eq!(editor.history(), ["version", "task list"]);
    }

    #[test]
    fn bufread_line_editor_recalls_history_with_up_and_down_sequences() {
        let mut stdin = std::io::Cursor::new(b"\x1b[A\n\x1b[B\n".to_vec());
        let mut editor = BufReadLineEditor::new(&mut stdin);
        editor.add_history_entry("version");
        editor.add_history_entry("help");
        let mut stdout = Vec::new();

        assert_eq!(
            editor.read_line(PROMPT, &mut stdout).unwrap(),
            LineRead::Line("help".to_string())
        );
        assert_eq!(
            editor.read_line(PROMPT, &mut stdout).unwrap(),
            LineRead::Line(String::new())
        );
    }

    struct VecLineEditor {
        reads: std::vec::IntoIter<LineRead>,
        history: Vec<String>,
    }

    impl VecLineEditor {
        fn new<const N: usize>(reads: [LineRead; N]) -> Self {
            Self {
                reads: reads.into_iter().collect::<Vec<_>>().into_iter(),
                history: Vec::new(),
            }
        }
    }

    impl LineEditor for VecLineEditor {
        fn read_line(&mut self, prompt: &str, stdout: &mut dyn Write) -> HarnessResult<LineRead> {
            write!(stdout, "{prompt}").unwrap();
            Ok(self.reads.next().unwrap_or(LineRead::Eof))
        }

        fn add_history_entry(&mut self, line: &str) {
            self.history.push(line.to_string());
        }

        fn history(&self) -> &[String] {
            &self.history
        }
    }

    #[derive(Default)]
    struct RecordingRunner {
        records: std::rc::Rc<RefCell<Vec<String>>>,
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
    }

    impl RecordingRunner {
        fn with_output(stdout: &str, stderr: &str, exit_code: Option<i32>) -> Self {
            Self {
                records: std::rc::Rc::new(RefCell::new(Vec::new())),
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
                exit_code,
            }
        }
    }

    impl CommandRunner for RecordingRunner {
        fn run_validation(&self, _spec: CommandSpec) -> HarnessResult<CommandOutput> {
            unreachable!("interactive shell escapes must not use validation runner")
        }

        fn run_shell_escape(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
            self.records.borrow_mut().push(spec.command);
            Ok(CommandOutput {
                stdout: self.stdout.clone(),
                stderr: self.stderr.clone(),
                exit_code: self.exit_code,
                duration_ms: 0,
                timed_out: false,
                truncated: false,
                truncated_bytes: 0,
            })
        }
    }

    struct StubService;

    impl StubService {
        fn unavailable(&self, method: &str) -> HarnessError {
            HarnessError::External(format!("{method} unavailable in interactive tests"))
        }
    }

    impl HarnessService for StubService {
        fn create_task(
            &self,
            _title: String,
            _goal: String,
            _validation_commands: Vec<String>,
        ) -> HarnessResult<Task> {
            Err(self.unavailable("create_task"))
        }

        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            Err(self.unavailable("list_tasks"))
        }

        fn get_task(&self, _task_id: &TaskId) -> HarnessResult<Task> {
            Err(self.unavailable("get_task"))
        }

        fn run_task(
            &self,
            _task_id: &TaskId,
            _options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            Err(self.unavailable("run_task"))
        }

        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            Err(self.unavailable("list_tickets"))
        }

        fn get_ticket(&self, _ticket_id: &TicketId) -> HarnessResult<Ticket> {
            Err(self.unavailable("get_ticket"))
        }

        fn resolve_ticket(
            &self,
            _ticket_id: &TicketId,
            _options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            Err(self.unavailable("resolve_ticket"))
        }

        fn resume_task(
            &self,
            _task_id: &TaskId,
            _options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            Err(self.unavailable("resume_task"))
        }
    }
}
