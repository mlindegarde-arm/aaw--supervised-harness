use crate::domain::{Task, TaskId, TaskStatus, Ticket, TicketId, TicketStatus};
use crate::error::{HarnessError, HarnessResult};
use crate::service::HarnessService;
use clap::{Arg, ArgAction, Command};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

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
}

impl CommandEvent {
    pub fn info(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            level: CommandEventLevel::Info,
            message: message.into(),
        }
    }

    pub fn warn(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            level: CommandEventLevel::Warn,
            message: message.into(),
        }
    }

    pub fn error(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            level: CommandEventLevel::Error,
            message: message.into(),
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

        writeln!(
            self.stderr,
            "{}: {}",
            event.level.as_str(),
            event.message.trim_end()
        )
        .map_err(io_error)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub path: &'static [&'static str],
    pub usage: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCatalog {
    commands: Vec<CommandSpec>,
}

impl CommandCatalog {
    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    pub fn help(&self) -> String {
        let mut help = format!(
            "harness {VERSION}\n\nUSAGE:\n    harness <command> [options]\n\nGLOBAL OPTIONS:\n    --output <human|json>\n    --quiet\n    --repo <path>\n    --state-dir <path>\n\nCOMMANDS:\n"
        );
        for command in &self.commands {
            help.push_str("    ");
            help.push_str(command.usage);
            help.push('\n');
        }
        help
    }
}

pub fn build_clap() -> Command {
    let globals = [
        Arg::new("output")
            .long("output")
            .value_parser(["human", "json"])
            .global(true),
        Arg::new("quiet")
            .long("quiet")
            .action(ArgAction::SetTrue)
            .global(true),
        Arg::new("repo")
            .long("repo")
            .value_name("path")
            .global(true),
        Arg::new("state-dir")
            .long("state-dir")
            .value_name("path")
            .global(true),
    ];

    Command::new("harness")
        .version(VERSION)
        .about("AI agent harness supervisor")
        .disable_help_subcommand(true)
        .args(globals)
        .subcommand(Command::new("init").arg(Arg::new("repo").long("repo").value_name("path")))
        .subcommand(
            Command::new("doctor")
                .arg(
                    Arg::new("offline")
                        .long("offline")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("providers")
                        .long("providers")
                        .value_parser(["local", "all"]),
                )
                .arg(Arg::new("deep").long("deep").action(ArgAction::SetTrue)),
        )
        .subcommand(
            Command::new("task")
                .subcommand(
                    Command::new("create")
                        .arg(Arg::new("title").long("title").required(true))
                        .arg(Arg::new("goal").long("goal").required(true))
                        .arg(
                            Arg::new("validation")
                                .long("validation")
                                .action(ArgAction::Append)
                                .required(true),
                        ),
                )
                .subcommand(Command::new("list").arg(Arg::new("status").long("status")))
                .subcommand(Command::new("get").arg(Arg::new("task-id").required(true)))
                .subcommand(
                    Command::new("run")
                        .arg(Arg::new("task-id").required(true))
                        .arg(Arg::new("max-attempts").long("max-attempts"))
                        .arg(Arg::new("model").long("model")),
                )
                .subcommand(
                    Command::new("cleanup")
                        .arg(Arg::new("task-id").required(true))
                        .arg(Arg::new("force").long("force").action(ArgAction::SetTrue))
                        .arg(
                            Arg::new("dry-run")
                                .long("dry-run")
                                .action(ArgAction::SetTrue),
                        ),
                ),
        )
        .subcommand(
            Command::new("ticket")
                .subcommand(Command::new("list").arg(Arg::new("status").long("status")))
                .subcommand(Command::new("get").arg(Arg::new("ticket-id").required(true)))
                .subcommand(
                    Command::new("resolve")
                        .arg(Arg::new("ticket-id").required(true))
                        .arg(Arg::new("model").long("model")),
                ),
        )
        .subcommand(
            Command::new("resume")
                .arg(Arg::new("task-id").required(true))
                .arg(Arg::new("ticket").long("ticket"))
                .arg(Arg::new("max-attempts").long("max-attempts"))
                .arg(Arg::new("model").long("model")),
        )
        .subcommand(
            Command::new("run")
                .arg(Arg::new("title").long("title").required(true))
                .arg(Arg::new("goal").long("goal").required(true))
                .arg(
                    Arg::new("validation")
                        .long("validation")
                        .action(ArgAction::Append)
                        .required(true),
                )
                .arg(Arg::new("max-attempts").long("max-attempts"))
                .arg(Arg::new("model").long("model")),
        )
        .subcommand(
            Command::new("config")
                .subcommand(Command::new("get"))
                .subcommand(
                    Command::new("set")
                        .arg(Arg::new("key").required(true))
                        .arg(Arg::new("value").required(true)),
                ),
        )
        .subcommand(
            Command::new("workspace").subcommand(
                Command::new("prune")
                    .arg(
                        Arg::new("dry-run")
                            .long("dry-run")
                            .action(ArgAction::SetTrue),
                    )
                    .arg(Arg::new("force").long("force").action(ArgAction::SetTrue)),
            ),
        )
}

pub fn build_cli() -> CommandCatalog {
    CommandCatalog {
        commands: vec![
            CommandSpec {
                path: &["init"],
                usage: "init [--repo <path>]",
            },
            CommandSpec {
                path: &["doctor"],
                usage: "doctor [--offline] [--providers local|all] [--deep]",
            },
            CommandSpec {
                path: &["task", "create"],
                usage: "task create --title <title> --goal <goal> --validation <cmd>...",
            },
            CommandSpec {
                path: &["task", "list"],
                usage: "task list [--status <status>]",
            },
            CommandSpec {
                path: &["task", "get"],
                usage: "task get <task-id>",
            },
            CommandSpec {
                path: &["task", "run"],
                usage: "task run <task-id> [--max-attempts <n>] [--model <ollama-model>]",
            },
            CommandSpec {
                path: &["task", "cleanup"],
                usage: "task cleanup <task-id> [--force] [--dry-run]",
            },
            CommandSpec {
                path: &["ticket", "list"],
                usage: "ticket list [--status <status>]",
            },
            CommandSpec {
                path: &["ticket", "get"],
                usage: "ticket get <ticket-id>",
            },
            CommandSpec {
                path: &["ticket", "resolve"],
                usage: "ticket resolve <ticket-id> [--model <openai-model>]",
            },
            CommandSpec {
                path: &["resume"],
                usage: "resume <task-id> [--ticket <ticket-id>] [--max-attempts <n>] [--model <ollama-model>]",
            },
            CommandSpec {
                path: &["run"],
                usage: "run --title <title> --goal <goal> --validation <cmd>... [--max-attempts <n>] [--model <model>]",
            },
            CommandSpec {
                path: &["config", "get"],
                usage: "config get",
            },
            CommandSpec {
                path: &["config", "set"],
                usage: "config set <key> <value>",
            },
            CommandSpec {
                path: &["workspace", "prune"],
                usage: "workspace prune [--dry-run] [--force]",
            },
        ],
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
                CommandExit::new(CommandStatus::Complete, 0, Some(self.catalog.help())),
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
            ParsedCommand::Init => placeholder("init is not wired until config integration"),
            ParsedCommand::Doctor { .. } => {
                placeholder("doctor is not wired until diagnostics integration")
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
            "task" => self.parse_task()?,
            "ticket" => self.parse_ticket()?,
            "resume" => self.parse_resume()?,
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
                        "--status" => status = Some(self.required_value("--status")?),
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
                        "--status" => status = Some(self.required_value("--status")?),
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
    use crate::domain::{RunId, RunStatus};
    use std::cell::RefCell;

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
        ];

        for command in commands {
            assert!(build_clap().try_get_matches_from(command).is_ok());
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

    #[derive(Default)]
    struct FakeService {
        created: RefCell<Vec<(String, String, Vec<String>)>>,
        run_requests: RefCell<Vec<(TaskId, TaskRunOptions)>>,
        resolve_requests: RefCell<Vec<(TicketId, TicketResolveOptions)>>,
        resume_requests: RefCell<Vec<(TaskId, ResumeTaskOptions)>>,
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
    }
}
