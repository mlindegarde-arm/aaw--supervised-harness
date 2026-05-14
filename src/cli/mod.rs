use crate::domain::{Task, TaskId, Ticket, TicketId};
use crate::error::{HarnessError, HarnessResult};
use crate::interactive;
use crate::runtime::{
    CommandExit, CommandResult, CommandRuntime, HumanSink, JsonSink, OutputMode, ResumeTaskOptions,
    TaskRunOptions, TicketResolveOptions,
};
use crate::service::{DefaultHarnessService, HarnessService};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn run<I, S>(args: I) -> CommandExit
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let tokens = args.iter().skip(1).cloned().collect::<Vec<_>>();
    if tokens.is_empty()
        && no_command_route(
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        ) == NoCommandRoute::Tui
    {
        return crate::tui::run_tui(command_service_factory(Vec::new()));
    }

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

    let service = command_service(&tokens);
    let runtime = CommandRuntime::new(service.as_ref());

    if tokens.is_empty() {
        return interactive::run_with_input(stdin, stdout, stderr, &runtime);
    }

    dispatch_tokens(tokens, stdout, stderr, &runtime)
}

fn command_service(tokens: &[String]) -> Box<dyn HarnessService> {
    match crate::config::load_config(repo_hint(tokens).as_deref()) {
        Ok(loaded) => match DefaultHarnessService::from_loaded_config(loaded) {
            Ok(service) => Box::new(service),
            Err(error) => Box::new(PlaceholderService::with_error(error)),
        },
        Err(error) => Box::new(PlaceholderService::with_error(error)),
    }
}

fn command_service_factory(tokens: Vec<String>) -> crate::tui::RuntimeServiceFactory {
    Arc::new(move || command_service(&tokens))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoCommandRoute {
    Tui,
    FallbackInteractive,
}

fn no_command_route(stdin_is_tty: bool, stdout_is_tty: bool) -> NoCommandRoute {
    if stdin_is_tty && stdout_is_tty {
        NoCommandRoute::Tui
    } else {
        NoCommandRoute::FallbackInteractive
    }
}

fn repo_hint(tokens: &[String]) -> Option<PathBuf> {
    tokens.iter().enumerate().find_map(|(index, token)| {
        if token == "--repo" {
            return tokens.get(index + 1).map(PathBuf::from);
        }
        token
            .strip_prefix("--repo=")
            .map(Path::new)
            .map(Path::to_path_buf)
    })
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

struct PlaceholderService {
    error: Option<String>,
}

impl PlaceholderService {
    fn with_error(error: HarnessError) -> Self {
        Self {
            error: Some(error.to_string()),
        }
    }

    fn unavailable(&self, method: &str) -> HarnessError {
        if let Some(error) = &self.error {
            return HarnessError::External(format!("{method} is unavailable: {error}"));
        }
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

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        assert_eq!(stdout.lines().count(), 1);
        let value: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(value["status"], "complete");
        assert!(value["data"]["tasks"].is_array());
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
    fn cli_no_command_route_uses_tui_only_for_tty_input_and_output() {
        assert_eq!(no_command_route(true, true), NoCommandRoute::Tui);
        assert_eq!(
            no_command_route(false, true),
            NoCommandRoute::FallbackInteractive
        );
        assert_eq!(
            no_command_route(true, false),
            NoCommandRoute::FallbackInteractive
        );
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
