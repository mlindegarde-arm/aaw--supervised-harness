use crate::domain::{Task, TaskId, Ticket, TicketId};
use crate::error::{HarnessError, HarnessResult};
use crate::runtime::{
    CommandExit, CommandResult, CommandRuntime, HumanSink, JsonSink, OutputMode, ResumeTaskOptions,
    TaskRunOptions, TicketResolveOptions,
};
use crate::security::{DefaultEnvironmentSanitizer, EnvironmentSanitizer};
use crate::service::HarnessService;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

pub fn run<I, S>(args: I) -> CommandExit
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    run_with_input(args, &mut stdin, &mut stdout, &mut stderr)
}

pub fn run_with_io<I, S>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> CommandExit
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut stdin = std::io::Cursor::new(Vec::<u8>::new());
    run_with_input(args, &mut stdin, stdout, stderr)
}

pub fn run_with_input<I, S>(
    args: I,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> CommandExit
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let tokens = args.into_iter().skip(1).collect::<Vec<_>>();

    let service = PlaceholderService;
    let runtime = CommandRuntime::new(&service);

    if tokens.is_empty() {
        return run_interactive(stdin, stdout, stderr, &runtime);
    }

    dispatch_tokens(tokens, stdout, stderr, &runtime)
}

fn dispatch_tokens(
    tokens: Vec<String>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    runtime: &CommandRuntime<'_>,
) -> CommandExit {
    let output_mode = output_mode_hint(&tokens).unwrap_or(OutputMode::Human);
    let quiet = tokens.iter().any(|token| token == "--quiet");

    match output_mode {
        OutputMode::Human => {
            let mut sink = HumanSink::new(stdout, stderr, quiet);
            runtime.dispatch(tokens, &mut sink)
        }
        OutputMode::Json => {
            let mut sink = JsonSink::new(stdout, stderr, quiet);
            runtime.dispatch(tokens, &mut sink)
        }
    }
}

fn run_interactive(
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    runtime: &CommandRuntime<'_>,
) -> CommandExit {
    if let Err(err) = writeln!(
        stdout,
        "harness interactive mode; type exit or quit to leave."
    ) {
        return CommandExit::failure(format!("failed to write command output: {err}"));
    }

    let mut line = String::new();
    loop {
        if let Err(err) = write!(stdout, "harness> ") {
            return CommandExit::failure(format!("failed to write command output: {err}"));
        }
        if let Err(err) = stdout.flush() {
            return CommandExit::failure(format!("failed to write command output: {err}"));
        }

        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => return CommandExit::success(),
            Ok(_) => {}
            Err(err) => {
                return CommandExit::failure(format!("failed to read command input: {err}"));
            }
        }

        let command = line.trim();
        if command.is_empty() {
            continue;
        }
        if command == "exit" || command == "quit" {
            return CommandExit::success();
        }
        if let Some(shell_command) = command.strip_prefix('!') {
            run_shell_escape(shell_command.trim_start(), stdout, stderr);
            continue;
        }

        let output_mode = output_mode_hint_line(command).unwrap_or(OutputMode::Human);
        let quiet = command.split_whitespace().any(|token| token == "--quiet");
        match output_mode {
            OutputMode::Human => {
                let mut sink = HumanSink::new(stdout, stderr, quiet);
                runtime.dispatch_line(command, &mut sink)
            }
            OutputMode::Json => {
                let mut sink = JsonSink::new(stdout, stderr, quiet);
                runtime.dispatch_line(command, &mut sink)
            }
        };
    }
}

fn run_shell_escape(command: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) {
    if command.trim().is_empty() {
        let _ = writeln!(stderr, "shell escape command cannot be empty");
        return;
    }

    let cwd = discover_repo_root_for_shell_escape().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let env = std::env::vars().collect::<BTreeMap<_, _>>();
    let env = DefaultEnvironmentSanitizer::new().sanitize(&env);

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .output();

    match output {
        Ok(output) => {
            let _ = stdout.write_all(&output.stdout);
            let _ = stderr.write_all(&output.stderr);
            if !output.status.success() {
                let code = output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string());
                let _ = writeln!(stderr, "shell escape exited with {code}");
            }
        }
        Err(err) => {
            let _ = writeln!(stderr, "failed to run shell escape: {err}");
        }
    }
}

fn discover_repo_root_for_shell_escape() -> Option<std::path::PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8(output.stdout).ok()?;
    let root = root.trim();
    (!root.is_empty()).then(|| std::path::PathBuf::from(root))
}

fn output_mode_hint(tokens: &[String]) -> Option<OutputMode> {
    tokens.iter().enumerate().find_map(|(index, token)| {
        if token == "--output" {
            return tokens
                .get(index + 1)
                .and_then(|value| match value.as_str() {
                    "human" => Some(OutputMode::Human),
                    "json" => Some(OutputMode::Json),
                    _ => None,
                });
        }

        token
            .strip_prefix("--output=")
            .and_then(|value| match value {
                "human" => Some(OutputMode::Human),
                "json" => Some(OutputMode::Json),
                _ => None,
            })
    })
}

fn output_mode_hint_line(line: &str) -> Option<OutputMode> {
    let tokens = crate::runtime::tokenize_shell_like(line).ok()?;
    output_mode_hint(&tokens)
}

struct PlaceholderService;

impl PlaceholderService {
    fn unavailable(&self, method: &str) -> HarnessError {
        HarnessError::External(format!(
            "{method} is not wired yet; service integration belongs to the orchestrator workstream"
        ))
    }
}

impl HarnessService for PlaceholderService {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn cli_run_uses_json_sink_when_output_json_is_requested() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = run_with_io(
            ["harness", "--output", "json", "task", "list"],
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(exit.code(), 1);
        let stdout = String::from_utf8(stdout).unwrap();
        assert_eq!(stdout.lines().count(), 1);
        let value: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(value["status"], "failed");
    }

    #[test]
    fn cli_run_does_not_exit_on_parse_error() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = run_with_io(["harness", "task", "nope"], &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 2);
        assert!(stdout.is_empty());
        assert!(!stderr.is_empty());
    }

    #[test]
    fn cli_no_args_enters_interactive_runtime() {
        let mut stdin = std::io::Cursor::new(b"version\nexit\n".to_vec());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = run_with_input(["harness"], &mut stdin, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.contains("interactive mode"));
        assert!(stdout.contains(&format!("harness {}", env!("CARGO_PKG_VERSION"))));
        assert!(stderr.is_empty());
    }

    #[test]
    fn cli_interactive_keeps_shell_open_after_invalid_command() {
        let mut stdin = std::io::Cursor::new(b"task nope\nversion\nexit\n".to_vec());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = run_with_input(["harness"], &mut stdin, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stdout.contains(&format!("harness {}", env!("CARGO_PKG_VERSION"))));
        assert!(!stderr.is_empty());
    }

    #[test]
    fn cli_interactive_shell_escape_bypasses_runtime_and_keeps_shell_open() {
        let mut stdin = std::io::Cursor::new(b"! printf shell-ok\nversion\nexit\n".to_vec());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = run_with_input(["harness"], &mut stdin, &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        assert!(stdout.contains("shell-ok"));
        assert!(stdout.contains(&format!("harness {}", env!("CARGO_PKG_VERSION"))));
        assert!(stderr.is_empty());
    }

    #[test]
    fn cli_version_returns_command_exit() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = run_with_io(["harness", "--version"], &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            format!("harness {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(stderr.is_empty());
    }
}
