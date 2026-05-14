pub mod catalog;
pub mod events;
pub mod supervise;

use crate::doctor::{DiagnosticStatus, DoctorOptions, DoctorReport};
use crate::domain::{Task, TaskId, TaskStatus, Ticket, TicketId, TicketStatus};
use crate::error::{HarnessError, HarnessResult};
use crate::service::HarnessService;
use crate::state::SqliteTaskStore;
use clap::builder::PossibleValuesParser;
use clap::{Arg, ArgAction, Command};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

pub use catalog::{
    CommandAction, CommandCatalog, CommandNodeSpec, CommandSpec, CommandTreeSpec,
    MetaCommandAction, MetaCommandSpec, OptionAction, OptionSpec, PositionalSpec, StateQueryKind,
    ValueKind, ValueSource, ValueSpec, build_cli, phase2_command_tree_seed,
};
pub use events::{
    PaneArtifactRow, PaneRunRow, PaneSection, PaneStateSnapshot, PaneTaskRow, PaneTicketRow,
    SuperviseProgressEvent, SuperviseProgressPhase, TranscriptEvent, TuiRuntimeEvent,
};
pub use supervise::{
    CancellationToken, CooperativeCancellationToken, SuperviseCreateOptions, SuperviseTaskOptions,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    Complete,
    Failed,
    Usage,
    Stuck,
    Leased,
    DoctorFailed,
    SecurityBlocked,
}

impl CommandStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Usage => "usage",
            Self::Stuck => "stuck",
            Self::Leased => "leased",
            Self::DoctorFailed => "doctor_failed",
            Self::SecurityBlocked => "security_blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExit {
    pub status: CommandStatus,
    pub exit_code: u8,
    pub message: Option<String>,
}

impl CommandExit {
    pub fn new(status: CommandStatus, exit_code: u8, message: Option<String>) -> Self {
        Self {
            status,
            exit_code,
            message,
        }
    }

    pub fn success() -> Self {
        Self::new(CommandStatus::Complete, 0, None)
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self::new(CommandStatus::Failed, 1, Some(message.into()))
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(CommandStatus::Usage, 2, Some(message.into()))
    }

    pub fn stuck(message: impl Into<String>) -> Self {
        Self::new(CommandStatus::Stuck, 10, Some(message.into()))
    }

    pub fn leased(message: impl Into<String>) -> Self {
        Self::new(CommandStatus::Leased, 11, Some(message.into()))
    }

    pub fn doctor_failed(message: impl Into<String>) -> Self {
        Self::new(CommandStatus::DoctorFailed, 20, Some(message.into()))
    }

    pub fn security_blocked(message: impl Into<String>) -> Self {
        Self::new(CommandStatus::SecurityBlocked, 30, Some(message.into()))
    }

    pub fn code(&self) -> u8 {
        self.exit_code
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandResult {
    pub exit: CommandExit,
    pub events: Vec<CommandEvent>,
    pub data: Value,
}

impl CommandResult {
    pub fn new(exit: CommandExit) -> Self {
        Self {
            exit,
            events: Vec::new(),
            data: Value::Null,
        }
    }

    pub fn with_data(exit: CommandExit, data: Value) -> Self {
        Self {
            exit,
            events: Vec::new(),
            data,
        }
    }

    pub fn with_event(mut self, event: CommandEvent) -> Self {
        self.events.push(event);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEvent {
    pub kind: String,
    pub level: CommandEventLevel,
    pub message: String,
    pub supervise_progress: Option<SuperviseProgressEvent>,
}

impl CommandEvent {
    pub fn info(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            level: CommandEventLevel::Info,
            message: message.into(),
            supervise_progress: None,
        }
    }

    pub fn warn(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            level: CommandEventLevel::Warn,
            message: message.into(),
            supervise_progress: None,
        }
    }

    pub fn error(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            level: CommandEventLevel::Error,
            message: message.into(),
            supervise_progress: None,
        }
    }

    pub fn supervise_progress(event: SuperviseProgressEvent, level: CommandEventLevel) -> Self {
        Self {
            kind: "supervise.phase".to_string(),
            level,
            message: event.message.clone(),
            supervise_progress: Some(event),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEventLevel {
    Info,
    Warn,
    Error,
}

impl CommandEventLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

pub trait OutputSink {
    fn event(&mut self, event: &CommandEvent) -> HarnessResult<()>;
    fn finish(&mut self, result: &CommandResult) -> HarnessResult<()>;
}

pub struct HumanSink<'a> {
    stdout: &'a mut dyn Write,
    stderr: &'a mut dyn Write,
    quiet: bool,
}

impl<'a> HumanSink<'a> {
    pub fn new(stdout: &'a mut dyn Write, stderr: &'a mut dyn Write, quiet: bool) -> Self {
        Self {
            stdout,
            stderr,
            quiet,
        }
    }
}

impl OutputSink for HumanSink<'_> {
    fn event(&mut self, event: &CommandEvent) -> HarnessResult<()> {
        if self.quiet && event.level == CommandEventLevel::Info {
            return Ok(());
        }

        let writer = if event.level == CommandEventLevel::Error {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        writeln!(
            writer,
            "{}: {}",
            event.level.as_str(),
            event.message.trim_end()
        )
        .map_err(io_error)
    }

    fn finish(&mut self, result: &CommandResult) -> HarnessResult<()> {
        if let Some(message) = &result.exit.message {
            let writer = if result.exit.exit_code == 0 {
                &mut self.stdout
            } else {
                &mut self.stderr
            };
            writeln!(writer, "{message}").map_err(io_error)?;
        }
        Ok(())
    }
}

pub struct JsonSink<'a> {
    stdout: &'a mut dyn Write,
    stderr: &'a mut dyn Write,
    quiet: bool,
}

impl<'a> JsonSink<'a> {
    pub fn new(stdout: &'a mut dyn Write, stderr: &'a mut dyn Write, quiet: bool) -> Self {
        Self {
            stdout,
            stderr,
            quiet,
        }
    }
}

impl OutputSink for JsonSink<'_> {
    fn event(&mut self, event: &CommandEvent) -> HarnessResult<()> {
        if self.quiet && event.level == CommandEventLevel::Info {
            return Ok(());
        }

        if let Some(progress) = &event.supervise_progress {
            serde_json::to_writer(&mut self.stderr, &progress.to_json()).map_err(|err| {
                HarnessError::External(format!("failed to write JSON event output: {err}"))
            })?;
            writeln!(self.stderr).map_err(io_error)
        } else {
            writeln!(
                self.stderr,
                "{}: {}",
                event.level.as_str(),
                event.message.trim_end()
            )
            .map_err(io_error)
        }
    }

    fn finish(&mut self, result: &CommandResult) -> HarnessResult<()> {
        let mut object = json!({
            "status": result.exit.status.as_str(),
            "exit_code": result.exit.exit_code,
        });

        if let Some(message) = &result.exit.message {
            object["message"] = json!(message);
        }
        if !result.data.is_null() {
            object["data"] = result.data.clone();
        }

        serde_json::to_writer(&mut self.stdout, &object)
            .map_err(|err| HarnessError::External(format!("failed to write JSON output: {err}")))?;
        writeln!(self.stdout).map_err(io_error)
    }
}

pub struct InteractiveSink<'a> {
    inner: HumanSink<'a>,
}

impl<'a> InteractiveSink<'a> {
    pub fn new(stdout: &'a mut dyn Write, stderr: &'a mut dyn Write, quiet: bool) -> Self {
        Self {
            inner: HumanSink::new(stdout, stderr, quiet),
        }
    }

    pub fn write_stdout(&mut self, bytes: &[u8]) -> HarnessResult<()> {
        self.inner.stdout.write_all(bytes).map_err(io_error)
    }

    pub fn write_stderr(&mut self, bytes: &[u8]) -> HarnessResult<()> {
        self.inner.stderr.write_all(bytes).map_err(io_error)
    }

    pub fn write_stderr_line(&mut self, message: &str) -> HarnessResult<()> {
        writeln!(self.inner.stderr, "{message}").map_err(io_error)
    }
}

impl OutputSink for InteractiveSink<'_> {
    fn event(&mut self, event: &CommandEvent) -> HarnessResult<()> {
        self.inner.event(event)
    }

    fn finish(&mut self, result: &CommandResult) -> HarnessResult<()> {
        self.inner.finish(result)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

impl OutputMode {
    fn parse(value: &str) -> Result<Self, ParseFailure> {
        match value {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            other => Err(ParseFailure::usage(format!(
                "invalid --output value {other:?}; expected human or json"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub output: OutputMode,
    pub quiet: bool,
    pub repo: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            output: OutputMode::Human,
            quiet: false,
            repo: None,
            state_dir: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRunOptions {
    pub runtime: RuntimeOptions,
    pub max_attempts: Option<u32>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketResolveOptions {
    pub runtime: RuntimeOptions,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTaskOptions {
    pub runtime: RuntimeOptions,
    pub ticket_id: Option<TicketId>,
    pub max_attempts: Option<u32>,
    pub model: Option<String>,
}

pub fn build_clap() -> Command {
    let tree = phase2_command_tree_seed();
    let mut command = Command::new(tree.name)
        .version(VERSION)
        .about("AI agent harness supervisor")
        .disable_help_subcommand(true);
    for option in tree.globals {
        command = command.arg(clap_arg_from_option(option).global(true));
    }
    for child in tree.commands {
        command = command.subcommand(clap_command_from_spec(child));
    }
    command
}

fn clap_command_from_spec(spec: &'static CommandNodeSpec) -> Command {
    let mut command = Command::new(spec.name).about(spec.about);
    for alias in spec.aliases {
        command = command.alias(alias);
    }
    if spec.hidden {
        command = command.hide(true);
    }
    for positional in spec.positionals {
        command = command.arg(clap_arg_from_positional(positional));
    }
    for option in spec.options {
        command = command.arg(clap_arg_from_option(option));
    }
    for child in spec.children {
        command = command.subcommand(clap_command_from_spec(child));
    }
    command
}

fn clap_arg_from_positional(spec: &'static PositionalSpec) -> Arg {
    let mut arg = Arg::new(spec.name).required(spec.required);
    for required_unless_present in spec.required_unless_present {
        arg = arg.required_unless_present(required_unless_present);
    }
    for conflict in spec.conflicts_with {
        arg = arg.conflicts_with(conflict);
    }
    if spec.repeatable {
        arg = arg.action(ArgAction::Append);
    }
    apply_value_parser(arg, &spec.value)
}

fn clap_arg_from_option(spec: &'static OptionSpec) -> Arg {
    let mut arg = Arg::new(spec.long).long(spec.long).required(spec.required);
    if let Some(short) = spec.short {
        arg = arg.short(short);
    }
    if let Some(value_name) = spec.value_name {
        arg = arg.value_name(value_name);
    }
    arg = match spec.action {
        OptionAction::Set => arg.action(ArgAction::Set),
        OptionAction::SetTrue => arg.action(ArgAction::SetTrue),
        OptionAction::Append => arg.action(ArgAction::Append),
    };
    for required in spec.requires {
        arg = arg.requires(required);
    }
    for conflict in spec.conflicts_with {
        arg = arg.conflicts_with(conflict);
    }
    if let Some(value) = &spec.value {
        apply_value_parser(arg, value)
    } else {
        arg
    }
}

fn apply_value_parser(arg: Arg, value: &ValueSpec) -> Arg {
    match (&value.kind, &value.source) {
        (_, ValueSource::Static(values)) => {
            arg.value_parser(PossibleValuesParser::new(values.iter().copied()))
        }
        _ => arg,
    }
}

pub struct CommandRuntime<'a> {
    service: &'a dyn HarnessService,
    catalog: CommandCatalog,
}

impl<'a> CommandRuntime<'a> {
    pub fn new(service: &'a dyn HarnessService) -> Self {
        Self {
            service,
            catalog: build_cli(),
        }
    }

    pub fn dispatch<I, S>(&self, args: I, sink: &mut dyn OutputSink) -> CommandExit
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
        self.dispatch_tokens(args, sink)
    }

    pub fn dispatch_line(&self, line: &str, sink: &mut dyn OutputSink) -> CommandExit {
        match tokenize_shell_like(line) {
            Ok(tokens) => self.dispatch_tokens(tokens, sink),
            Err(err) => finish_sink(sink, CommandResult::new(err.into_exit())),
        }
    }

    fn dispatch_tokens(&self, mut tokens: Vec<String>, sink: &mut dyn OutputSink) -> CommandExit {
        if tokens.first().is_some_and(|token| token == "harness") {
            tokens.remove(0);
        }

        let parsed = match parse_command(tokens) {
            Ok(parsed) => parsed,
            Err(err) => return finish_sink(sink, CommandResult::new(err.into_exit())),
        };

        let runtime_options = parsed.options;
        let result = match parsed.command {
            ParsedCommand::Help => CommandResult::with_data(
                CommandExit::new(CommandStatus::Complete, 0, Some(self.catalog.help(VERSION))),
                json!({ "version": VERSION }),
            ),
            ParsedCommand::Version => CommandResult::with_data(
                CommandExit::new(
                    CommandStatus::Complete,
                    0,
                    Some(format!("harness {VERSION}")),
                ),
                json!({ "version": VERSION }),
            ),
            ParsedCommand::Init => init_result(runtime_options.repo.as_deref()),
            ParsedCommand::Doctor {
                offline,
                providers,
                deep,
            } => doctor_result(crate::doctor::run_doctor(DoctorOptions::from_cli(
                runtime_options.repo.clone(),
                runtime_options.state_dir.clone(),
                offline,
                providers.as_deref(),
                deep,
            ))),
            ParsedCommand::Completions { shell } => {
                match crate::completion::shell::completion_script(shell) {
                    Ok(script) => CommandResult::new(CommandExit::new(
                        CommandStatus::Complete,
                        0,
                        Some(script),
                    )),
                    Err(err) => error_result(err),
                }
            }
            ParsedCommand::TaskCreate {
                title,
                goal,
                validation,
            } => match self.service.create_task(title, goal, validation) {
                Ok(task) => task_result(task),
                Err(err) => error_result(err),
            },
            ParsedCommand::TaskList { status } => match self.service.list_tasks() {
                Ok(tasks) => {
                    let tasks = filter_tasks(tasks, status.as_deref());
                    CommandResult::with_data(
                        CommandExit::new(
                            CommandStatus::Complete,
                            0,
                            Some(format!("{} task(s)", tasks.len())),
                        ),
                        json!({
                            "tasks": tasks.iter().map(task_json).collect::<Vec<_>>(),
                            "next": "harness task get <task-id>",
                        }),
                    )
                }
                Err(err) => error_result(err),
            },
            ParsedCommand::TaskGet { task_id } => match self.service.get_task(&task_id) {
                Ok(task) => task_result(task),
                Err(err) => error_result(err),
            },
            ParsedCommand::TaskRun {
                task_id,
                max_attempts,
                model,
            } => {
                let options = TaskRunOptions {
                    runtime: runtime_options,
                    max_attempts,
                    model,
                };
                match self.service.run_task(&task_id, options) {
                    Ok(result) => result,
                    Err(err) => error_result(err),
                }
            }
            ParsedCommand::TaskCleanup { .. } => {
                placeholder("task cleanup is not wired until workspace integration")
            }
            ParsedCommand::TicketList { status } => match self.service.list_tickets() {
                Ok(tickets) => {
                    let tickets = filter_tickets(tickets, status.as_deref());
                    CommandResult::with_data(
                        CommandExit::new(
                            CommandStatus::Complete,
                            0,
                            Some(format!("{} ticket(s)", tickets.len())),
                        ),
                        json!({
                            "tickets": tickets.iter().map(ticket_json).collect::<Vec<_>>(),
                            "next": "harness ticket get <ticket-id>",
                        }),
                    )
                }
                Err(err) => error_result(err),
            },
            ParsedCommand::TicketGet { ticket_id } => match self.service.get_ticket(&ticket_id) {
                Ok(ticket) => ticket_result(ticket),
                Err(err) => error_result(err),
            },
            ParsedCommand::TicketResolve { ticket_id, model } => {
                let options = TicketResolveOptions {
                    runtime: runtime_options,
                    model,
                };
                match self.service.resolve_ticket(&ticket_id, options) {
                    Ok(result) => result,
                    Err(err) => error_result(err),
                }
            }
            ParsedCommand::Resume {
                task_id,
                ticket_id,
                max_attempts,
                model,
            } => {
                let options = ResumeTaskOptions {
                    runtime: runtime_options,
                    ticket_id,
                    max_attempts,
                    model,
                };
                match self.service.resume_task(&task_id, options) {
                    Ok(result) => result,
                    Err(err) => error_result(err),
                }
            }
            ParsedCommand::SuperviseTask {
                task_id,
                ticket_id,
                max_attempts,
                model,
                ticket_model,
                max_cycles,
            } => {
                let options = SuperviseTaskOptions {
                    runtime: runtime_options,
                    ticket_id,
                    max_attempts,
                    model,
                    ticket_model,
                    max_cycles,
                };
                let result = match self
                    .service
                    .supervise_task_streaming(&task_id, options, sink)
                {
                    Ok(result) => result,
                    Err(err) => error_result(err),
                };
                return finish_sink(sink, result);
            }
            ParsedCommand::SuperviseCreate {
                title,
                goal,
                validation,
                max_attempts,
                model,
                ticket_model,
                max_cycles,
            } => {
                let mut options = SuperviseCreateOptions::new(title, goal, validation);
                options.runtime = runtime_options;
                options.max_attempts = max_attempts;
                options.model = model;
                options.ticket_model = ticket_model;
                options.max_cycles = max_cycles;
                let result = match self
                    .service
                    .create_and_supervise_task_streaming(options, sink)
                {
                    Ok(result) => result,
                    Err(err) => error_result(err),
                };
                return finish_sink(sink, result);
            }
            ParsedCommand::Run {
                title,
                goal,
                validation,
                max_attempts,
                model,
            } => match self.service.create_task(title, goal, validation) {
                Ok(task) => match self.service.run_task(
                    &task.id,
                    TaskRunOptions {
                        runtime: runtime_options,
                        max_attempts,
                        model,
                    },
                ) {
                    Ok(result) => result,
                    Err(err) => error_result(err),
                },
                Err(err) => error_result(err),
            },
            ParsedCommand::ConfigGet => {
                placeholder("config get is not wired until config integration")
            }
            ParsedCommand::ConfigSet { .. } => {
                placeholder("config set is not wired until config integration")
            }
            ParsedCommand::WorkspacePrune { .. } => {
                placeholder("workspace prune is not wired until workspace integration")
            }
        };

        for event in &result.events {
            if let Err(err) = sink.event(event) {
                return CommandExit::failure(err.to_string());
            }
        }
        finish_sink(sink, result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    Help,
    Version,
    Init,
    Doctor {
        offline: bool,
        providers: Option<String>,
        deep: bool,
    },
    Completions {
        shell: crate::completion::shell::Shell,
    },
    TaskCreate {
        title: String,
        goal: String,
        validation: Vec<String>,
    },
    TaskList {
        status: Option<String>,
    },
    TaskGet {
        task_id: TaskId,
    },
    TaskRun {
        task_id: TaskId,
        max_attempts: Option<u32>,
        model: Option<String>,
    },
    TaskCleanup {
        task_id: TaskId,
        force: bool,
        dry_run: bool,
    },
    TicketList {
        status: Option<String>,
    },
    TicketGet {
        ticket_id: TicketId,
    },
    TicketResolve {
        ticket_id: TicketId,
        model: Option<String>,
    },
    Resume {
        task_id: TaskId,
        ticket_id: Option<TicketId>,
        max_attempts: Option<u32>,
        model: Option<String>,
    },
    SuperviseTask {
        task_id: TaskId,
        ticket_id: Option<TicketId>,
        max_attempts: Option<u32>,
        model: Option<String>,
        ticket_model: Option<String>,
        max_cycles: Option<u32>,
    },
    SuperviseCreate {
        title: String,
        goal: String,
        validation: Vec<String>,
        max_attempts: Option<u32>,
        model: Option<String>,
        ticket_model: Option<String>,
        max_cycles: Option<u32>,
    },
    Run {
        title: String,
        goal: String,
        validation: Vec<String>,
        max_attempts: Option<u32>,
        model: Option<String>,
    },
    ConfigGet,
    ConfigSet {
        key: String,
        value: String,
    },
    WorkspacePrune {
        dry_run: bool,
        force: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInvocation {
    pub options: RuntimeOptions,
    pub command: ParsedCommand,
}

pub fn parse_command(tokens: Vec<String>) -> Result<ParsedInvocation, ParseFailure> {
    let mut parser = Parser::new(tokens);
    let command = parser.parse()?;
    Ok(ParsedInvocation {
        options: parser.options,
        command,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseFailure {
    message: String,
}

impl ParseFailure {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn into_exit(self) -> CommandExit {
        CommandExit::usage(self.message)
    }
}

struct Parser {
    tokens: Vec<String>,
    index: usize,
    options: RuntimeOptions,
}

impl Parser {
    fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens,
            index: 0,
            options: RuntimeOptions::default(),
        }
    }

    fn parse(&mut self) -> Result<ParsedCommand, ParseFailure> {
        if self.tokens.is_empty() {
            return Ok(ParsedCommand::Help);
        }

        let command = self.next_command_token()?;
        let parsed = match command.as_str() {
            "-h" | "--help" | "help" => ParsedCommand::Help,
            "-V" | "--version" | "version" => ParsedCommand::Version,
            "init" => ParsedCommand::Init,
            "doctor" => self.parse_doctor()?,
            "completions" => self.parse_completions()?,
            "task" => self.parse_task()?,
            "ticket" => self.parse_ticket()?,
            "resume" => self.parse_resume()?,
            "supervise" => self.parse_supervise()?,
            "run" => self.parse_run()?,
            "config" => self.parse_config()?,
            "workspace" => self.parse_workspace()?,
            other => return Err(ParseFailure::usage(format!("unknown command {other:?}"))),
        };
        self.reject_trailing()?;
        Ok(parsed)
    }

    fn parse_doctor(&mut self) -> Result<ParsedCommand, ParseFailure> {
        let mut offline = false;
        let mut providers = None;
        let mut deep = false;
        while let Some(token) = self.next_option_or_none()? {
            match token.as_str() {
                "--offline" => offline = true,
                "--deep" => deep = true,
                "--providers" => {
                    let value = self.required_value("--providers")?;
                    if value != "local" && value != "all" {
                        return Err(ParseFailure::usage(
                            "--providers must be either local or all",
                        ));
                    }
                    providers = Some(value);
                }
                other => {
                    return Err(ParseFailure::usage(format!(
                        "unknown doctor option {other:?}"
                    )));
                }
            }
        }
        Ok(ParsedCommand::Doctor {
            offline,
            providers,
            deep,
        })
    }

    fn parse_completions(&mut self) -> Result<ParsedCommand, ParseFailure> {
        let shell = self.required_value("shell")?;
        let shell = crate::completion::shell::Shell::parse(&shell).map_err(ParseFailure::usage)?;
        Ok(ParsedCommand::Completions { shell })
    }

    fn parse_task(&mut self) -> Result<ParsedCommand, ParseFailure> {
        match self.required_command("task subcommand")?.as_str() {
            "create" => {
                let mut title = None;
                let mut goal = None;
                let mut validation = Vec::new();
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--title" => title = Some(self.required_value("--title")?),
                        "--goal" => goal = Some(self.required_value("--goal")?),
                        "--validation" => validation.push(self.required_value("--validation")?),
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown task create option {other:?}"
                            )));
                        }
                    }
                }
                let title = title.ok_or_else(|| ParseFailure::usage("missing --title"))?;
                let goal = goal.ok_or_else(|| ParseFailure::usage("missing --goal"))?;
                if validation.is_empty() {
                    return Err(ParseFailure::usage(
                        "task create requires at least one --validation command",
                    ));
                }
                Ok(ParsedCommand::TaskCreate {
                    title,
                    goal,
                    validation,
                })
            }
            "list" => {
                let mut status = None;
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--status" => {
                            let value = self.required_value("--status")?;
                            validate_task_status(&value)?;
                            status = Some(value);
                        }
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown task list option {other:?}"
                            )));
                        }
                    }
                }
                Ok(ParsedCommand::TaskList { status })
            }
            "get" => Ok(ParsedCommand::TaskGet {
                task_id: TaskId::parse(self.required_value("task-id")?)
                    .map_err(|err| ParseFailure::usage(err.to_string()))?,
            }),
            "run" => {
                let task_id = TaskId::parse(self.required_value("task-id")?)
                    .map_err(|err| ParseFailure::usage(err.to_string()))?;
                let mut max_attempts = None;
                let mut model = None;
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--max-attempts" => {
                            max_attempts = Some(parse_u32(
                                "--max-attempts",
                                self.required_value("--max-attempts")?,
                            )?)
                        }
                        "--model" => model = Some(self.required_value("--model")?),
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown task run option {other:?}"
                            )));
                        }
                    }
                }
                Ok(ParsedCommand::TaskRun {
                    task_id,
                    max_attempts,
                    model,
                })
            }
            "cleanup" => {
                let task_id = TaskId::parse(self.required_value("task-id")?)
                    .map_err(|err| ParseFailure::usage(err.to_string()))?;
                let mut force = false;
                let mut dry_run = false;
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--force" => force = true,
                        "--dry-run" => dry_run = true,
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown task cleanup option {other:?}"
                            )));
                        }
                    }
                }
                Ok(ParsedCommand::TaskCleanup {
                    task_id,
                    force,
                    dry_run,
                })
            }
            other => Err(ParseFailure::usage(format!(
                "unknown task subcommand {other:?}"
            ))),
        }
    }

    fn parse_ticket(&mut self) -> Result<ParsedCommand, ParseFailure> {
        match self.required_command("ticket subcommand")?.as_str() {
            "list" => {
                let mut status = None;
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--status" => {
                            let value = self.required_value("--status")?;
                            validate_ticket_status(&value)?;
                            status = Some(value);
                        }
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown ticket list option {other:?}"
                            )));
                        }
                    }
                }
                Ok(ParsedCommand::TicketList { status })
            }
            "get" => Ok(ParsedCommand::TicketGet {
                ticket_id: TicketId::parse(self.required_value("ticket-id")?)
                    .map_err(|err| ParseFailure::usage(err.to_string()))?,
            }),
            "resolve" => {
                let ticket_id = TicketId::parse(self.required_value("ticket-id")?)
                    .map_err(|err| ParseFailure::usage(err.to_string()))?;
                let mut model = None;
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--model" => model = Some(self.required_value("--model")?),
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown ticket resolve option {other:?}"
                            )));
                        }
                    }
                }
                Ok(ParsedCommand::TicketResolve { ticket_id, model })
            }
            other => Err(ParseFailure::usage(format!(
                "unknown ticket subcommand {other:?}"
            ))),
        }
    }

    fn parse_resume(&mut self) -> Result<ParsedCommand, ParseFailure> {
        let task_id = TaskId::parse(self.required_value("task-id")?)
            .map_err(|err| ParseFailure::usage(err.to_string()))?;
        let mut ticket_id = None;
        let mut max_attempts = None;
        let mut model = None;
        while let Some(token) = self.next_option_or_none()? {
            match token.as_str() {
                "--ticket" => {
                    ticket_id = Some(
                        TicketId::parse(self.required_value("--ticket")?)
                            .map_err(|err| ParseFailure::usage(err.to_string()))?,
                    )
                }
                "--max-attempts" => {
                    max_attempts = Some(parse_u32(
                        "--max-attempts",
                        self.required_value("--max-attempts")?,
                    )?)
                }
                "--model" => model = Some(self.required_value("--model")?),
                other => {
                    return Err(ParseFailure::usage(format!(
                        "unknown resume option {other:?}"
                    )));
                }
            }
        }
        Ok(ParsedCommand::Resume {
            task_id,
            ticket_id,
            max_attempts,
            model,
        })
    }

    fn parse_supervise(&mut self) -> Result<ParsedCommand, ParseFailure> {
        let mut task_id = None;
        let mut create = false;
        let mut title = None;
        let mut goal = None;
        let mut validation = Vec::new();
        let mut ticket_id = None;
        let mut max_attempts = None;
        let mut model = None;
        let mut ticket_model = None;
        let mut max_cycles = None;

        while let Some(token) = self.next_option_or_none()? {
            match token.as_str() {
                "--create" => create = true,
                "--title" => title = Some(self.required_value("--title")?),
                "--goal" => goal = Some(self.required_value("--goal")?),
                "--validation" => validation.push(self.required_value("--validation")?),
                "--ticket" => {
                    ticket_id = Some(
                        TicketId::parse(self.required_value("--ticket")?)
                            .map_err(|err| ParseFailure::usage(err.to_string()))?,
                    )
                }
                "--max-attempts" => {
                    max_attempts = Some(parse_u32(
                        "--max-attempts",
                        self.required_value("--max-attempts")?,
                    )?)
                }
                "--model" => model = Some(self.required_value("--model")?),
                "--ticket-model" => ticket_model = Some(self.required_value("--ticket-model")?),
                "--max-cycles" => {
                    max_cycles = Some(parse_u32(
                        "--max-cycles",
                        self.required_value("--max-cycles")?,
                    )?)
                }
                other if other.starts_with('-') => {
                    return Err(ParseFailure::usage(format!(
                        "unknown supervise option {other:?}"
                    )));
                }
                other => {
                    if task_id.is_some() {
                        return Err(ParseFailure::usage(format!(
                            "unexpected argument {other:?}"
                        )));
                    }
                    task_id = Some(
                        TaskId::parse(other).map_err(|err| ParseFailure::usage(err.to_string()))?,
                    );
                }
            }
        }

        if create {
            if task_id.is_some() {
                return Err(ParseFailure::usage(
                    "supervise --create cannot be combined with <task-id>",
                ));
            }
            if ticket_id.is_some() {
                return Err(ParseFailure::usage(
                    "supervise --create cannot be combined with --ticket",
                ));
            }
            let title = title.ok_or_else(|| ParseFailure::usage("missing --title"))?;
            let goal = goal.ok_or_else(|| ParseFailure::usage("missing --goal"))?;
            if validation.is_empty() {
                return Err(ParseFailure::usage(
                    "supervise --create requires at least one --validation command",
                ));
            }
            Ok(ParsedCommand::SuperviseCreate {
                title,
                goal,
                validation,
                max_attempts,
                model,
                ticket_model,
                max_cycles,
            })
        } else {
            if title.is_some() || goal.is_some() || !validation.is_empty() {
                return Err(ParseFailure::usage(
                    "supervise create fields require --create",
                ));
            }
            let task_id =
                task_id.ok_or_else(|| ParseFailure::usage("missing task-id or --create"))?;
            Ok(ParsedCommand::SuperviseTask {
                task_id,
                ticket_id,
                max_attempts,
                model,
                ticket_model,
                max_cycles,
            })
        }
    }

    fn parse_run(&mut self) -> Result<ParsedCommand, ParseFailure> {
        let mut title = None;
        let mut goal = None;
        let mut validation = Vec::new();
        let mut max_attempts = None;
        let mut model = None;
        while let Some(token) = self.next_option_or_none()? {
            match token.as_str() {
                "--title" => title = Some(self.required_value("--title")?),
                "--goal" => goal = Some(self.required_value("--goal")?),
                "--validation" => validation.push(self.required_value("--validation")?),
                "--max-attempts" => {
                    max_attempts = Some(parse_u32(
                        "--max-attempts",
                        self.required_value("--max-attempts")?,
                    )?)
                }
                "--model" => model = Some(self.required_value("--model")?),
                other => return Err(ParseFailure::usage(format!("unknown run option {other:?}"))),
            }
        }
        let title = title.ok_or_else(|| ParseFailure::usage("missing --title"))?;
        let goal = goal.ok_or_else(|| ParseFailure::usage("missing --goal"))?;
        if validation.is_empty() {
            return Err(ParseFailure::usage(
                "run requires at least one --validation command",
            ));
        }
        Ok(ParsedCommand::Run {
            title,
            goal,
            validation,
            max_attempts,
            model,
        })
    }

    fn parse_config(&mut self) -> Result<ParsedCommand, ParseFailure> {
        match self.required_command("config subcommand")?.as_str() {
            "get" => Ok(ParsedCommand::ConfigGet),
            "set" => Ok(ParsedCommand::ConfigSet {
                key: self.required_value("key")?,
                value: self.required_value("value")?,
            }),
            other => Err(ParseFailure::usage(format!(
                "unknown config subcommand {other:?}"
            ))),
        }
    }

    fn parse_workspace(&mut self) -> Result<ParsedCommand, ParseFailure> {
        match self.required_command("workspace subcommand")?.as_str() {
            "prune" => {
                let mut dry_run = false;
                let mut force = false;
                while let Some(token) = self.next_option_or_none()? {
                    match token.as_str() {
                        "--dry-run" => dry_run = true,
                        "--force" => force = true,
                        other => {
                            return Err(ParseFailure::usage(format!(
                                "unknown workspace prune option {other:?}"
                            )));
                        }
                    }
                }
                Ok(ParsedCommand::WorkspacePrune { dry_run, force })
            }
            other => Err(ParseFailure::usage(format!(
                "unknown workspace subcommand {other:?}"
            ))),
        }
    }

    fn next_command_token(&mut self) -> Result<String, ParseFailure> {
        loop {
            let Some(token) = self.next_raw() else {
                return Ok("help".to_string());
            };
            if self.consume_global(token.as_str())? {
                continue;
            }
            return Ok(token);
        }
    }

    fn required_command(&mut self, name: &str) -> Result<String, ParseFailure> {
        self.next_command_token().and_then(|value| {
            if value == "help" {
                Err(ParseFailure::usage(format!("missing {name}")))
            } else {
                Ok(value)
            }
        })
    }

    fn next_option_or_none(&mut self) -> Result<Option<String>, ParseFailure> {
        loop {
            let Some(token) = self.next_raw() else {
                return Ok(None);
            };
            if self.consume_global(token.as_str())? {
                continue;
            }
            return Ok(Some(token));
        }
    }

    fn required_value(&mut self, name: &str) -> Result<String, ParseFailure> {
        loop {
            let Some(token) = self.next_raw() else {
                return Err(ParseFailure::usage(format!("missing value for {name}")));
            };
            if self.consume_global(token.as_str())? {
                continue;
            }
            return Ok(token);
        }
    }

    fn reject_trailing(&mut self) -> Result<(), ParseFailure> {
        while let Some(token) = self.next_raw() {
            if self.consume_global(token.as_str())? {
                continue;
            }
            return Err(ParseFailure::usage(format!(
                "unexpected argument {token:?}"
            )));
        }
        Ok(())
    }

    fn consume_global(&mut self, token: &str) -> Result<bool, ParseFailure> {
        match token {
            "--quiet" => {
                self.options.quiet = true;
                Ok(true)
            }
            "--output" => {
                let value = self
                    .next_raw()
                    .ok_or_else(|| ParseFailure::usage("missing value for --output"))?;
                self.options.output = OutputMode::parse(&value)?;
                Ok(true)
            }
            "--repo" => {
                let value = self
                    .next_raw()
                    .ok_or_else(|| ParseFailure::usage("missing value for --repo"))?;
                self.options.repo = Some(PathBuf::from(value));
                Ok(true)
            }
            "--state-dir" => {
                let value = self
                    .next_raw()
                    .ok_or_else(|| ParseFailure::usage("missing value for --state-dir"))?;
                self.options.state_dir = Some(PathBuf::from(value));
                Ok(true)
            }
            _ if token.starts_with("--output=") => {
                self.options.output = OutputMode::parse(&token["--output=".len()..])?;
                Ok(true)
            }
            _ if token.starts_with("--repo=") => {
                self.options.repo = Some(PathBuf::from(&token["--repo=".len()..]));
                Ok(true)
            }
            _ if token.starts_with("--state-dir=") => {
                self.options.state_dir = Some(PathBuf::from(&token["--state-dir=".len()..]));
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn next_raw(&mut self) -> Option<String> {
        let token = self.tokens.get(self.index)?.clone();
        self.index += 1;
        Some(token)
    }
}

pub fn tokenize_shell_like(input: &str) -> Result<Vec<String>, ParseFailure> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote = None;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push('\\');
                    }
                }
                _ => current.push(ch),
            },
            Some(_) => unreachable!(),
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    token_started = true;
                }
                '\\' => {
                    token_started = true;
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push('\\');
                    }
                }
                ch if ch.is_whitespace() => {
                    if token_started {
                        tokens.push(std::mem::take(&mut current));
                        token_started = false;
                    }
                }
                _ => {
                    token_started = true;
                    current.push(ch);
                }
            },
        }
    }

    if let Some(quote) = quote {
        return Err(ParseFailure::usage(format!("unterminated {quote} quote")));
    }
    if token_started {
        tokens.push(current);
    }
    Ok(tokens)
}

fn parse_u32(name: &str, value: String) -> Result<u32, ParseFailure> {
    value
        .parse::<u32>()
        .map_err(|_| ParseFailure::usage(format!("{name} must be a positive integer")))
}

fn validate_task_status(value: &str) -> Result<(), ParseFailure> {
    TaskStatus::parse(value)
        .map(|_| ())
        .map_err(|err| ParseFailure::usage(err.to_string()))
}

fn validate_ticket_status(value: &str) -> Result<(), ParseFailure> {
    TicketStatus::parse(value)
        .map(|_| ())
        .map_err(|err| ParseFailure::usage(err.to_string()))
}

fn finish_sink(sink: &mut dyn OutputSink, result: CommandResult) -> CommandExit {
    let exit = result.exit.clone();
    if let Err(err) = sink.finish(&result) {
        return CommandExit::failure(err.to_string());
    }
    exit
}

fn io_error(err: std::io::Error) -> HarnessError {
    HarnessError::External(format!("failed to write command output: {err}"))
}

fn placeholder(message: &'static str) -> CommandResult {
    CommandResult::new(CommandExit::failure(message))
        .with_event(CommandEvent::warn("placeholder", message))
}

fn init_result(repo: Option<&std::path::Path>) -> CommandResult {
    match crate::config::init_repo(repo) {
        Ok(init) => match SqliteTaskStore::open(&init.paths.state_file) {
            Ok(_) => CommandResult::with_data(
                CommandExit::new(
                    CommandStatus::Complete,
                    0,
                    Some("harness initialized".to_string()),
                ),
                json!({
                    "repo_root": init.paths.repo_root,
                    "state_dir": init.paths.state_dir,
                    "config_file": init.paths.config_file,
                    "state_file": init.paths.state_file,
                    "logs_dir": init.paths.logs_dir,
                    "artifacts_dir": init.paths.artifacts_dir,
                    "worktree_root": init.paths.worktree_root,
                    "config_created": init.config_created,
                    "harness_gitignore_warning": init.harness_gitignore_warning,
                }),
            ),
            Err(err) => error_result(err),
        },
        Err(err) => error_result(err),
    }
}

fn error_result(error: HarnessError) -> CommandResult {
    let exit = match &error {
        HarnessError::Usage(_)
        | HarnessError::InvalidId { .. }
        | HarnessError::InvalidStatus { .. }
        | HarnessError::InvalidConfig(_) => CommandExit::usage(error.to_string()),
        HarnessError::SecurityPolicy(_) => CommandExit::security_blocked(error.to_string()),
        HarnessError::Conflict(_) => CommandExit::leased(error.to_string()),
        HarnessError::NotFound { .. } | HarnessError::External(_) => {
            CommandExit::failure(error.to_string())
        }
    };
    CommandResult::new(exit)
}

fn doctor_result(report: DoctorReport) -> CommandResult {
    let exit = if report.has_failures() {
        CommandExit::doctor_failed(report.message())
    } else {
        CommandExit::new(CommandStatus::Complete, 0, Some(report.message()))
    };
    let checks = report.checks.clone();
    let data = serde_json::to_value(&report).unwrap_or_else(|err| {
        json!({
            "serialization_error": err.to_string(),
        })
    });

    checks
        .into_iter()
        .fold(CommandResult::with_data(exit, data), |result, check| {
            result.with_event(doctor_event(&check))
        })
}

fn doctor_event(check: &crate::doctor::DiagnosticCheck) -> CommandEvent {
    let message = format!(
        "{} {}: {}",
        check.status.as_str().to_ascii_uppercase(),
        check.label,
        check.message
    );
    match check.status {
        DiagnosticStatus::Pass | DiagnosticStatus::Skipped => {
            CommandEvent::info(check.id.clone(), message)
        }
        DiagnosticStatus::Warn => CommandEvent::warn(check.id.clone(), message),
        DiagnosticStatus::Fail => CommandEvent::error(check.id.clone(), message),
    }
}

fn task_result(task: Task) -> CommandResult {
    CommandResult::with_data(
        CommandExit::new(
            CommandStatus::Complete,
            0,
            Some(format!("task {}", task.id.as_str())),
        ),
        json!({
            "task_id": task.id.as_str(),
            "task": task_json(&task),
            "next": format!("harness task run {}", task.id.as_str()),
        }),
    )
}

fn ticket_result(ticket: Ticket) -> CommandResult {
    CommandResult::with_data(
        CommandExit::new(
            CommandStatus::Complete,
            0,
            Some(format!("ticket {}", ticket.id.as_str())),
        ),
        json!({
            "ticket_id": ticket.id.as_str(),
            "ticket": ticket_json(&ticket),
            "next": format!("harness ticket resolve {}", ticket.id.as_str()),
        }),
    )
}

fn filter_tasks(tasks: Vec<Task>, status: Option<&str>) -> Vec<Task> {
    match status.and_then(|status| TaskStatus::parse(status).ok()) {
        Some(status) => tasks
            .into_iter()
            .filter(|task| task.status == status)
            .collect(),
        None => tasks,
    }
}

fn filter_tickets(tickets: Vec<Ticket>, status: Option<&str>) -> Vec<Ticket> {
    match status.and_then(|status| TicketStatus::parse(status).ok()) {
        Some(status) => tickets
            .into_iter()
            .filter(|ticket| ticket.status == status)
            .collect(),
        None => tickets,
    }
}

fn task_json(task: &Task) -> Value {
    json!({
        "id": task.id.as_str(),
        "title": task.title,
        "goal": task.goal,
        "status": task.status.as_str(),
        "repo_root": task.repo_root,
        "worktree_path": task.worktree_path,
        "branch": task.branch,
        "base_ref": task.base_ref,
        "base_commit": task.base_commit,
        "last_seen_head": task.last_seen_head,
        "max_attempts": task.max_attempts,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
    })
}

fn ticket_json(ticket: &Ticket) -> Value {
    json!({
        "id": ticket.id.as_str(),
        "task_id": ticket.task_id.as_str(),
        "run_id": ticket.run_id.as_str(),
        "status": ticket.status.as_str(),
        "blocked_on": ticket.blocked_on,
        "question": ticket.question,
        "reason": ticket.reason,
        "created_at": ticket.created_at,
        "resolved_at": ticket.resolved_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::domain::{RunId, RunStatus};
    use crate::providers::{FakeHttpResponse, FakeHttpRoute, FakeHttpServer};
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;
    use std::process::Command as ProcessCommand;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn command_exit_codes_match_design() {
        assert_eq!(CommandExit::success().code(), 0);
        assert_eq!(CommandExit::failure("failed").code(), 1);
        assert_eq!(CommandExit::usage("usage").code(), 2);
        assert_eq!(CommandExit::stuck("stuck").code(), 10);
        assert_eq!(CommandExit::leased("leased").code(), 11);
        assert_eq!(CommandExit::doctor_failed("doctor").code(), 20);
        assert_eq!(CommandExit::security_blocked("security").code(), 30);
    }

    #[test]
    fn cli_catalog_contains_documented_commands() {
        let catalog = build_cli();
        let paths = catalog
            .commands()
            .iter()
            .map(|command| command.path.join(" "))
            .collect::<Vec<_>>();

        for expected in [
            "init",
            "doctor",
            "completions",
            "task create",
            "task list",
            "task get",
            "task run",
            "task cleanup",
            "ticket list",
            "ticket get",
            "ticket resolve",
            "resume",
            "run",
            "config get",
            "config set",
            "workspace prune",
            "supervise",
        ] {
            assert!(paths.iter().any(|path| path == expected), "{expected}");
        }
    }

    #[test]
    fn parser_preserves_quoted_validation_commands_and_globals() {
        let tokens = tokenize_shell_like(
            r#"harness task create --title "Fix add" --goal 'Make tests pass' --validation "cargo test cli runtime" --output=json --quiet"#,
        )
        .unwrap();
        let parsed = parse_command(tokens.into_iter().skip(1).collect()).unwrap();

        assert_eq!(parsed.options.output, OutputMode::Json);
        assert!(parsed.options.quiet);
        assert_eq!(
            parsed.command,
            ParsedCommand::TaskCreate {
                title: "Fix add".to_string(),
                goal: "Make tests pass".to_string(),
                validation: vec!["cargo test cli runtime".to_string()],
            }
        );
    }

    #[test]
    fn parser_rejects_missing_validation() {
        let err = parse_command(vec![
            "task".to_string(),
            "create".to_string(),
            "--title".to_string(),
            "No validation".to_string(),
            "--goal".to_string(),
            "Do it".to_string(),
        ])
        .unwrap_err();

        assert!(err.message.contains("--validation"));
    }

    #[test]
    fn parser_accepts_documented_command_surface() {
        let commands = [
            vec!["init", "--repo", "/repo", "--state-dir", "/state"],
            vec!["doctor", "--offline", "--providers", "local", "--deep"],
            vec!["completions", "bash"],
            vec![
                "task",
                "create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
            ],
            vec!["task", "list", "--status", "ready"],
            vec!["task", "get", TASK_ID],
            vec![
                "task",
                "run",
                TASK_ID,
                "--max-attempts",
                "2",
                "--model",
                "m",
            ],
            vec!["task", "cleanup", TASK_ID, "--force", "--dry-run"],
            vec!["ticket", "list", "--status", "open"],
            vec!["ticket", "get", TICKET_ID],
            vec!["ticket", "resolve", TICKET_ID, "--model", "gpt"],
            vec![
                "resume",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "2",
                "--model",
                "m",
            ],
            vec![
                "run",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--max-attempts",
                "2",
                "--model",
                "m",
            ],
            vec!["config", "get"],
            vec!["config", "set", "providers.ollama.default_model", "coder"],
            vec!["workspace", "prune", "--dry-run", "--force"],
            vec![
                "supervise",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "2",
                "--model",
                "m",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "3",
            ],
            vec![
                "supervise",
                "--create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--max-attempts",
                "2",
                "--model",
                "m",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "3",
            ],
        ];

        for command in commands {
            let tokens = command.into_iter().map(str::to_string).collect::<Vec<_>>();
            assert!(parse_command(tokens).is_ok());
        }
    }

    #[test]
    fn clap_command_tree_accepts_documented_commands() {
        let commands = [
            vec!["harness", "init", "--repo", "/repo"],
            vec![
                "harness",
                "doctor",
                "--offline",
                "--providers",
                "all",
                "--deep",
            ],
            vec!["harness", "completions", "zsh"],
            vec![
                "harness",
                "task",
                "create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
            ],
            vec!["harness", "task", "list", "--status", "ready"],
            vec!["harness", "task", "get", TASK_ID],
            vec![
                "harness",
                "task",
                "run",
                TASK_ID,
                "--max-attempts",
                "2",
                "--model",
                "m",
            ],
            vec![
                "harness",
                "task",
                "cleanup",
                TASK_ID,
                "--force",
                "--dry-run",
            ],
            vec!["harness", "ticket", "list", "--status", "open"],
            vec!["harness", "ticket", "get", TICKET_ID],
            vec!["harness", "ticket", "resolve", TICKET_ID, "--model", "gpt"],
            vec![
                "harness",
                "resume",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "2",
                "--model",
                "m",
            ],
            vec![
                "harness",
                "run",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--max-attempts",
                "2",
                "--model",
                "m",
            ],
            vec!["harness", "config", "get"],
            vec![
                "harness",
                "config",
                "set",
                "providers.ollama.default_model",
                "coder",
            ],
            vec!["harness", "workspace", "prune", "--dry-run", "--force"],
            vec![
                "harness",
                "supervise",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "2",
                "--model",
                "m",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "3",
            ],
            vec![
                "harness",
                "supervise",
                "--create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--max-attempts",
                "2",
                "--model",
                "m",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "3",
            ],
        ];

        for command in commands {
            assert!(build_clap().try_get_matches_from(command).is_ok());
        }
    }

    #[test]
    fn parser_and_clap_reject_invalid_static_schema_values() {
        for command in [
            vec!["task", "list", "--status", "wat"],
            vec!["ticket", "list", "--status", "wat"],
            vec!["completions", "powershell"],
        ] {
            let err = parse_command(command.into_iter().map(str::to_string).collect()).unwrap_err();
            assert_eq!(err.into_exit().code(), 2);
        }

        for command in [
            vec!["harness", "task", "list", "--status", "wat"],
            vec!["harness", "ticket", "list", "--status", "wat"],
            vec!["harness", "completions", "powershell"],
        ] {
            assert!(
                build_clap().try_get_matches_from(command).is_err(),
                "clap accepted invalid static schema value"
            );
        }
    }

    #[test]
    fn clap_rejects_invalid_supervise_create_parity_cases() {
        for command in [
            vec!["harness", "supervise"],
            vec![
                "harness",
                "supervise",
                "--create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
            ],
            vec![
                "harness",
                "supervise",
                "--create",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
            ],
            vec![
                "harness",
                "supervise",
                "--create",
                "--title",
                "Fix",
                "--validation",
                "cargo test",
            ],
            vec![
                "harness",
                "supervise",
                TASK_ID,
                "--create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
            ],
            vec!["harness", "supervise", TASK_ID, "--title", "Fix"],
            vec![
                "harness",
                "supervise",
                "--create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--ticket",
                TICKET_ID,
            ],
        ] {
            assert!(
                build_clap().try_get_matches_from(command.clone()).is_err(),
                "clap unexpectedly accepted {command:?}"
            );
        }
    }

    #[test]
    fn parser_accepts_documented_supervise_forms_and_globals() {
        let parsed = parse_command(
            [
                "--repo=/repo",
                "--output",
                "json",
                "supervise",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "2",
                "--model",
                "coder",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "3",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        )
        .unwrap();

        assert_eq!(parsed.options.output, OutputMode::Json);
        assert_eq!(parsed.options.repo, Some(PathBuf::from("/repo")));
        assert_eq!(
            parsed.command,
            ParsedCommand::SuperviseTask {
                task_id: TaskId::parse(TASK_ID).unwrap(),
                ticket_id: Some(TicketId::parse(TICKET_ID).unwrap()),
                max_attempts: Some(2),
                model: Some("coder".to_string()),
                ticket_model: Some("gpt".to_string()),
                max_cycles: Some(3),
            }
        );

        let parsed = parse_command(
            [
                "supervise",
                "--create",
                "--title",
                "Fix tests",
                "--goal",
                "Make cargo test pass",
                "--validation",
                "cargo test",
                "--validation",
                "cargo fmt --check",
                "--max-attempts",
                "4",
                "--model",
                "coder",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "5",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        )
        .unwrap();

        assert_eq!(
            parsed.command,
            ParsedCommand::SuperviseCreate {
                title: "Fix tests".to_string(),
                goal: "Make cargo test pass".to_string(),
                validation: vec!["cargo test".to_string(), "cargo fmt --check".to_string()],
                max_attempts: Some(4),
                model: Some("coder".to_string()),
                ticket_model: Some("gpt".to_string()),
                max_cycles: Some(5),
            }
        );
    }

    #[test]
    fn parser_rejects_invalid_supervise_forms() {
        for (command, expected) in [
            (vec!["supervise"], "missing task-id or --create"),
            (
                vec![
                    "supervise",
                    "--create",
                    "--goal",
                    "Goal",
                    "--validation",
                    "cargo test",
                ],
                "missing --title",
            ),
            (
                vec![
                    "supervise",
                    "--create",
                    "--title",
                    "Fix",
                    "--validation",
                    "cargo test",
                ],
                "missing --goal",
            ),
            (
                vec!["supervise", "--create", "--title", "Fix", "--goal", "Goal"],
                "--validation",
            ),
            (
                vec![
                    "supervise",
                    TASK_ID,
                    "--create",
                    "--title",
                    "Fix",
                    "--goal",
                    "Goal",
                    "--validation",
                    "cargo test",
                ],
                "cannot be combined with <task-id>",
            ),
            (
                vec!["supervise", "--title", "Fix"],
                "create fields require --create",
            ),
        ] {
            let err = parse_command(command.into_iter().map(str::to_string).collect()).unwrap_err();
            assert!(
                err.message.contains(expected),
                "expected {expected:?} in {:?}",
                err.message
            );
        }
    }

    #[test]
    fn help_catalog_exposes_metadata_and_supervise_surface() {
        let catalog = build_cli();
        let help = catalog.help(VERSION);

        assert!(help.contains("supervise <task-id> [--ticket <ticket-id>]"));
        assert!(
            help.contains("supervise --create --title <title> --goal <goal> --validation <cmd>..."),
            "{help}"
        );
        assert!(help.contains("INTERACTIVE COMMANDS:"));
        assert!(help.contains("exit - Exit the interactive UI."));
        assert!(help.contains("help (aliases: ?) - Show help."));

        let supervise = find_schema_command(&["supervise"]).unwrap();
        assert_eq!(supervise.action, CommandAction::Placeholder);
        assert!(supervise.options.iter().any(|option| {
            option.long == "validation"
                && option.repeatable
                && option.action == OptionAction::Append
                && option
                    .value
                    .as_ref()
                    .is_some_and(|value| value.kind == ValueKind::FreeText)
        }));
        assert!(supervise.options.iter().any(|option| {
            option.long == "ticket"
                && option.value.as_ref().is_some_and(|value| {
                    matches!(
                        value.source,
                        ValueSource::StateQuery(StateQueryKind::TicketId {
                            scoped_to_task_arg: true,
                            ..
                        })
                    )
                })
        }));
    }

    #[test]
    fn schema_clap_and_parser_command_surface_stay_in_parity() {
        let tree = phase2_command_tree_seed();
        let catalog = build_cli();
        let schema_paths = schema_leaf_paths(tree.commands);
        let catalog_paths = catalog
            .commands()
            .iter()
            .map(|command| command.path.clone())
            .collect::<Vec<_>>();

        for path in &schema_paths {
            assert!(
                catalog_paths
                    .iter()
                    .any(|catalog_path| catalog_path == path),
                "catalog missing schema path {path:?}"
            );
            assert!(
                clap_path_exists(&build_clap(), path),
                "clap missing schema path {path:?}"
            );
        }

        let clap_tree = build_clap();
        for option in tree.globals {
            let arg = clap_tree
                .get_arguments()
                .find(|arg| arg.get_long() == Some(option.long))
                .unwrap_or_else(|| panic!("missing global option {}", option.long));
            assert_eq!(
                clap_action_name(arg.get_action()),
                option_action_name(option.action)
            );
            assert_eq!(arg.is_required_set(), option.required);
        }

        for path in schema_paths {
            let schema = find_schema_command(&path).unwrap();
            let clap = find_clap_command(&clap_tree, &path).unwrap();
            for positional in schema.positionals {
                let arg = clap
                    .get_arguments()
                    .find(|arg| arg.get_id().as_str() == positional.name)
                    .unwrap_or_else(|| panic!("missing positional {:?} on {path:?}", positional));
                assert_eq!(arg.is_required_set(), positional.required);
            }
            for option in schema.options {
                let arg = clap
                    .get_arguments()
                    .find(|arg| arg.get_long() == Some(option.long))
                    .unwrap_or_else(|| panic!("missing option {} on {path:?}", option.long));
                assert_eq!(
                    clap_action_name(arg.get_action()),
                    option_action_name(option.action)
                );
                assert_eq!(arg.is_required_set(), option.required);
                assert_eq!(
                    clap_action_name(arg.get_action()) == "append",
                    option.repeatable,
                    "repeatability mismatch for --{} on {path:?}",
                    option.long
                );
                assert!(
                    option.value.is_some() == option.value_name.is_some(),
                    "value metadata mismatch for --{} on {path:?}",
                    option.long
                );
            }
        }
    }

    #[test]
    fn help_and_parse_errors_return_exits_without_panicking() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, false);

        assert_eq!(
            runtime.dispatch(["--help"], &mut sink).status,
            CommandStatus::Complete
        );

        let mut sink = HumanSink::new(&mut stdout, &mut stderr, false);
        let exit = runtime.dispatch(["task", "wat"], &mut sink);
        assert_eq!(exit.code(), 2);
    }

    #[test]
    fn version_returns_command_exit_without_exiting() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, false);

        let exit = runtime.dispatch(["--version"], &mut sink);

        assert_eq!(exit.status, CommandStatus::Complete);
        assert_eq!(exit.code(), 0);
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            format!("harness {VERSION}\n")
        );
        assert!(stderr.is_empty());
    }

    #[test]
    fn repeated_runtime_execution_reuses_process_and_service() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);

        for title in ["First", "Second"] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);
            let exit = runtime.dispatch(
                [
                    "task",
                    "create",
                    "--title",
                    title,
                    "--goal",
                    "goal",
                    "--validation",
                    "cargo test",
                ],
                &mut sink,
            );
            assert_eq!(exit.code(), 0);
        }

        assert_eq!(service.created.borrow().len(), 2);
    }

    #[test]
    fn json_sink_writes_exactly_one_stdout_object_and_events_to_stderr() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = JsonSink::new(&mut stdout, &mut stderr, false);

        let exit = runtime.dispatch(
            [
                "task",
                "run",
                TASK_ID,
                "--output",
                "json",
                "--max-attempts",
                "2",
            ],
            &mut sink,
        );

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        let stderr = String::from_utf8(stderr).unwrap();
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1, "{stdout:?}");
        let value: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["status"], "complete");
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["data"]["task_id"], TASK_ID);
        assert!(stderr.contains("running task"));
    }

    #[test]
    fn json_sink_writes_supervisor_progress_as_ndjson_to_stderr() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = JsonSink::new(&mut stdout, &mut stderr, false);

        let exit = runtime.dispatch(["supervise", TASK_ID, "--output", "json"], &mut sink);

        assert_eq!(exit.code(), 0);
        let stdout = String::from_utf8(stdout).unwrap();
        let stderr = String::from_utf8(stderr).unwrap();
        let stdout_lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(stdout_lines.len(), 1, "{stdout:?}");
        let stderr_lines = stderr.lines().collect::<Vec<_>>();
        assert_eq!(stderr_lines.len(), 1, "{stderr:?}");
        let event: Value = serde_json::from_str(stderr_lines[0]).unwrap();
        assert_eq!(event["event"], "supervise.phase");
        assert_eq!(event["phase"], "inspect");
        assert_eq!(event["task_id"], TASK_ID);
        assert_eq!(event["message"], "inspecting task");
        let final_object: Value = serde_json::from_str(stdout_lines[0]).unwrap();
        assert_eq!(final_object["status"], "complete");
        assert_eq!(final_object["data"]["task_id"], TASK_ID);
    }

    #[test]
    fn json_sink_quiet_suppresses_info_supervisor_progress_ndjson() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = JsonSink::new(&mut stdout, &mut stderr, true);

        let exit = runtime.dispatch(["supervise", TASK_ID, "--output", "json"], &mut sink);

        assert_eq!(exit.code(), 0);
        assert!(stderr.is_empty());
        assert_eq!(String::from_utf8(stdout).unwrap().lines().count(), 1);
    }

    #[test]
    fn task_run_passes_runtime_and_command_options_to_service() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);

        let exit = runtime.dispatch(
            [
                "--repo=/repo",
                "--state-dir",
                "/state",
                "task",
                "run",
                TASK_ID,
                "--max-attempts",
                "4",
                "--model",
                "coder",
            ],
            &mut sink,
        );

        assert_eq!(exit.code(), 0);
        let calls = service.run_requests.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_str(), TASK_ID);
        assert_eq!(calls[0].1.max_attempts, Some(4));
        assert_eq!(calls[0].1.model.as_deref(), Some("coder"));
        assert_eq!(calls[0].1.runtime.repo, Some(PathBuf::from("/repo")));
        assert_eq!(calls[0].1.runtime.state_dir, Some(PathBuf::from("/state")));
    }

    #[test]
    fn run_and_resume_do_not_drop_model_or_max_attempts() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);

        let exit = runtime.dispatch(
            [
                "run",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--max-attempts",
                "5",
                "--model",
                "coder",
            ],
            &mut sink,
        );
        assert_eq!(exit.code(), 0);

        let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);
        let exit = runtime.dispatch(
            [
                "resume",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "6",
                "--model",
                "resume-model",
            ],
            &mut sink,
        );
        assert_eq!(exit.code(), 0);

        let run_requests = service.run_requests.borrow();
        assert_eq!(run_requests.last().unwrap().1.max_attempts, Some(5));
        assert_eq!(
            run_requests.last().unwrap().1.model.as_deref(),
            Some("coder")
        );

        let resume_requests = service.resume_requests.borrow();
        assert_eq!(resume_requests.len(), 1);
        assert_eq!(resume_requests[0].0.as_str(), TASK_ID);
        assert_eq!(
            resume_requests[0]
                .1
                .ticket_id
                .as_ref()
                .map(TicketId::as_str),
            Some(TICKET_ID)
        );
        assert_eq!(resume_requests[0].1.max_attempts, Some(6));
        assert_eq!(resume_requests[0].1.model.as_deref(), Some("resume-model"));
    }

    #[test]
    fn ticket_resolve_passes_model_to_service() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);

        let exit = runtime.dispatch(
            ["ticket", "resolve", TICKET_ID, "--model", "gpt"],
            &mut sink,
        );

        assert_eq!(exit.code(), 0);
        let calls = service.resolve_requests.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_str(), TICKET_ID);
        assert_eq!(calls[0].1.model.as_deref(), Some("gpt"));
    }

    #[test]
    fn supervise_dispatch_passes_options_to_service_placeholders() {
        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);

        let exit = runtime.dispatch(
            [
                "--repo",
                "/repo",
                "supervise",
                TASK_ID,
                "--ticket",
                TICKET_ID,
                "--max-attempts",
                "2",
                "--model",
                "coder",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "3",
            ],
            &mut sink,
        );
        assert_eq!(exit.code(), 0);

        let calls = service.supervise_requests.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.as_str(), TASK_ID);
        assert_eq!(calls[0].1.runtime.repo, Some(PathBuf::from("/repo")));
        assert_eq!(
            calls[0].1.ticket_id.as_ref().map(TicketId::as_str),
            Some(TICKET_ID)
        );
        assert_eq!(calls[0].1.max_attempts, Some(2));
        assert_eq!(calls[0].1.model.as_deref(), Some("coder"));
        assert_eq!(calls[0].1.ticket_model.as_deref(), Some("gpt"));
        assert_eq!(calls[0].1.max_cycles, Some(3));

        let mut sink = HumanSink::new(&mut stdout, &mut stderr, true);
        let exit = runtime.dispatch(
            [
                "supervise",
                "--create",
                "--title",
                "Fix",
                "--goal",
                "Goal",
                "--validation",
                "cargo test",
                "--max-attempts",
                "4",
                "--model",
                "coder",
                "--ticket-model",
                "gpt",
                "--max-cycles",
                "5",
            ],
            &mut sink,
        );
        assert_eq!(exit.code(), 0);

        let calls = service.supervise_create_requests.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].title, "Fix");
        assert_eq!(calls[0].goal, "Goal");
        assert_eq!(calls[0].validation_commands, ["cargo test"]);
        assert_eq!(calls[0].max_attempts, Some(4));
        assert_eq!(calls[0].model.as_deref(), Some("coder"));
        assert_eq!(calls[0].ticket_model.as_deref(), Some("gpt"));
        assert_eq!(calls[0].max_cycles, Some(5));
    }

    #[test]
    fn doctor_readiness_failures_return_exit_code_20() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_doctor_runtime_repo(temp.path().join("repo"));
        config::init_repo(Some(&repo)).unwrap();
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/api/tags",
            FakeHttpResponse::json(500, serde_json::json!({"error": "not ready"})),
        )])
        .unwrap();
        let mut harness_config = config::default_config();
        harness_config.providers.ollama.base_url = server.base_url();
        harness_config.providers.ollama.default_model = "local-model".to_string();
        config::write_config(&repo, &harness_config).unwrap();

        let service = FakeService::default();
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = HumanSink::new(&mut stdout, &mut stderr, false);
        let repo_arg = repo.to_str().unwrap().to_string();

        let exit = runtime.dispatch(
            [
                "doctor",
                "--providers",
                "local",
                "--repo",
                repo_arg.as_str(),
            ],
            &mut sink,
        );

        assert_eq!(exit.status, CommandStatus::DoctorFailed);
        assert_eq!(exit.code(), 20);
        assert!(
            String::from_utf8(stderr)
                .unwrap()
                .contains("doctor providers_local failed")
        );
    }

    fn schema_leaf_paths(nodes: &'static [CommandNodeSpec]) -> Vec<Vec<&'static str>> {
        fn collect(
            nodes: &'static [CommandNodeSpec],
            prefix: Vec<&'static str>,
            paths: &mut Vec<Vec<&'static str>>,
        ) {
            for node in nodes {
                let mut path = prefix.clone();
                path.push(node.name);
                if node.children.is_empty() {
                    paths.push(path);
                } else {
                    collect(node.children, path, paths);
                }
            }
        }

        let mut paths = Vec::new();
        collect(nodes, Vec::new(), &mut paths);
        paths
    }

    fn find_schema_command(path: &[&str]) -> Option<&'static CommandNodeSpec> {
        fn find(
            nodes: &'static [CommandNodeSpec],
            path: &[&str],
        ) -> Option<&'static CommandNodeSpec> {
            let (head, tail) = path.split_first()?;
            let node = nodes.iter().find(|node| node.name == *head)?;
            if tail.is_empty() {
                Some(node)
            } else {
                find(node.children, tail)
            }
        }

        find(phase2_command_tree_seed().commands, path)
    }

    fn clap_path_exists(command: &clap::Command, path: &[&str]) -> bool {
        find_clap_command(command, path).is_some()
    }

    fn find_clap_command<'a>(
        command: &'a clap::Command,
        path: &[&str],
    ) -> Option<&'a clap::Command> {
        let (head, tail) = path.split_first()?;
        let child = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == *head)?;
        if tail.is_empty() {
            Some(child)
        } else {
            find_clap_command(child, tail)
        }
    }

    fn option_action_name(action: OptionAction) -> &'static str {
        match action {
            OptionAction::Set => "set",
            OptionAction::SetTrue => "set_true",
            OptionAction::Append => "append",
        }
    }

    fn clap_action_name(action: &ArgAction) -> &'static str {
        match format!("{action:?}").as_str() {
            "Set" => "set",
            "SetTrue" => "set_true",
            "Append" => "append",
            other => panic!("unexpected clap action {other}"),
        }
    }

    fn init_doctor_runtime_repo(repo: PathBuf) -> PathBuf {
        fs::create_dir_all(&repo).unwrap();
        run_doctor_runtime_git(&repo, &["init"]);
        run_doctor_runtime_git(&repo, &["config", "user.email", "doctor@example.invalid"]);
        run_doctor_runtime_git(&repo, &["config", "user.name", "Doctor Runtime Test"]);
        fs::write(repo.join("README.md"), "# doctor runtime test\n").unwrap();
        run_doctor_runtime_git(&repo, &["add", "."]);
        run_doctor_runtime_git(&repo, &["commit", "-m", "initial"]);
        repo
    }

    fn run_doctor_runtime_git(repo: &Path, args: &[&str]) {
        let output = ProcessCommand::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[derive(Default)]
    struct FakeService {
        created: RefCell<Vec<(String, String, Vec<String>)>>,
        run_requests: RefCell<Vec<(TaskId, TaskRunOptions)>>,
        resolve_requests: RefCell<Vec<(TicketId, TicketResolveOptions)>>,
        resume_requests: RefCell<Vec<(TaskId, ResumeTaskOptions)>>,
        supervise_requests: RefCell<Vec<(TaskId, SuperviseTaskOptions)>>,
        supervise_create_requests: RefCell<Vec<SuperviseCreateOptions>>,
    }

    impl HarnessService for FakeService {
        fn create_task(
            &self,
            title: String,
            goal: String,
            validation_commands: Vec<String>,
        ) -> HarnessResult<Task> {
            self.created
                .borrow_mut()
                .push((title.clone(), goal.clone(), validation_commands));
            Ok(Task {
                id: TaskId::parse(TASK_ID).unwrap(),
                title,
                goal,
                status: TaskStatus::Ready,
                repo_root: "/repo".to_string(),
                worktree_path: None,
                branch: None,
                base_ref: None,
                base_commit: None,
                last_seen_head: None,
                max_attempts: 3,
                lease_owner: None,
                lease_acquired_at: None,
                lease_expires_at: None,
                heartbeat_at: None,
                lock_version: 1,
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
            })
        }

        fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
            Ok(Vec::new())
        }

        fn get_task(&self, _task_id: &TaskId) -> HarnessResult<Task> {
            self.create_task(
                "title".to_string(),
                "goal".to_string(),
                vec!["cargo test".to_string()],
            )
        }

        fn run_task(
            &self,
            task_id: &TaskId,
            options: TaskRunOptions,
        ) -> HarnessResult<CommandResult> {
            self.run_requests
                .borrow_mut()
                .push((task_id.clone(), options));
            Ok(CommandResult::with_data(
                CommandExit::success(),
                json!({
                    "task_id": task_id.as_str(),
                    "run_id": RunId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap().as_str(),
                    "run_status": RunStatus::Complete.as_str(),
                    "next": format!("harness task get {}", task_id.as_str()),
                }),
            )
            .with_event(CommandEvent::info("run", "running task")))
        }

        fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
            Ok(Vec::new())
        }

        fn get_ticket(&self, _ticket_id: &TicketId) -> HarnessResult<Ticket> {
            Ok(Ticket {
                id: TicketId::parse(TICKET_ID).unwrap(),
                task_id: TaskId::parse(TASK_ID).unwrap(),
                run_id: RunId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
                status: TicketStatus::Open,
                blocked_on: "validation".to_string(),
                question: "What next?".to_string(),
                reason: "stuck".to_string(),
                evidence_json: "{}".to_string(),
                failure_fingerprint: "abc".to_string(),
                created_at: "now".to_string(),
                resolved_at: None,
            })
        }

        fn resolve_ticket(
            &self,
            ticket_id: &TicketId,
            options: TicketResolveOptions,
        ) -> HarnessResult<CommandResult> {
            self.resolve_requests
                .borrow_mut()
                .push((ticket_id.clone(), options));
            Ok(CommandResult::new(CommandExit::success()))
        }

        fn resume_task(
            &self,
            task_id: &TaskId,
            options: ResumeTaskOptions,
        ) -> HarnessResult<CommandResult> {
            self.resume_requests
                .borrow_mut()
                .push((task_id.clone(), options));
            Ok(CommandResult::new(CommandExit::success()))
        }

        fn supervise_task(
            &self,
            task_id: &TaskId,
            options: SuperviseTaskOptions,
        ) -> HarnessResult<CommandResult> {
            self.supervise_requests
                .borrow_mut()
                .push((task_id.clone(), options));
            let progress = SuperviseProgressEvent {
                phase: SuperviseProgressPhase::InspectTask,
                task_id: Some(task_id.clone()),
                run_id: None,
                ticket_id: None,
                cycle: Some(0),
                message: "inspecting task".to_string(),
                next_command: Some(format!("harness task get {} --output json", task_id)),
            };
            Ok(CommandResult::with_data(
                CommandExit::success(),
                json!({ "task_id": task_id.as_str() }),
            )
            .with_event(CommandEvent::supervise_progress(
                progress,
                CommandEventLevel::Info,
            )))
        }

        fn create_and_supervise_task(
            &self,
            options: SuperviseCreateOptions,
        ) -> HarnessResult<CommandResult> {
            self.supervise_create_requests.borrow_mut().push(options);
            Ok(CommandResult::with_data(
                CommandExit::success(),
                json!({ "task_id": TASK_ID }),
            ))
        }
    }
}
