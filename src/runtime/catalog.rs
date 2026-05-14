#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTreeSpec {
    pub name: &'static str,
    pub globals: &'static [OptionSpec],
    pub commands: &'static [CommandNodeSpec],
    pub meta_commands: &'static [MetaCommandSpec],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandNodeSpec {
    pub name: &'static str,
    pub about: &'static str,
    pub aliases: &'static [&'static str],
    pub hidden: bool,
    pub examples: &'static [&'static str],
    pub children: &'static [CommandNodeSpec],
    pub positionals: &'static [PositionalSpec],
    pub options: &'static [OptionSpec],
    pub action: CommandAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaCommandSpec {
    pub name: &'static str,
    pub about: &'static str,
    pub aliases: &'static [&'static str],
    pub action: MetaCommandAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionalSpec {
    pub name: &'static str,
    pub value: ValueSpec,
    pub required: bool,
    pub required_unless_present: &'static [&'static str],
    pub repeatable: bool,
    pub conflicts_with: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionSpec {
    pub long: &'static str,
    pub short: Option<char>,
    pub value_name: Option<&'static str>,
    pub value: Option<ValueSpec>,
    pub required: bool,
    pub repeatable: bool,
    pub action: OptionAction,
    pub requires: &'static [&'static str],
    pub conflicts_with: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueSpec {
    pub kind: ValueKind,
    pub source: ValueSource,
    pub help: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    OutputMode,
    ProviderScope,
    Shell,
    TaskStatus,
    TicketStatus,
    TaskId,
    TicketId,
    Path,
    Model,
    FreeText,
    PositiveInteger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueSource {
    Static(&'static [&'static str]),
    StateQuery(StateQueryKind),
    FilesystemPath,
    HintOnly,
    NoCompletion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateQueryKind {
    TaskId {
        statuses: &'static [&'static str],
    },
    TicketId {
        statuses: &'static [&'static str],
        scoped_to_task_arg: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    Dispatch,
    Placeholder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaCommandAction {
    Exit,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionAction {
    Set,
    SetTrue,
    Append,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub path: Vec<&'static str>,
    pub usage: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCatalog {
    commands: Vec<CommandSpec>,
    tree: CommandTreeSpec,
}

impl CommandCatalog {
    pub fn from_tree(tree: CommandTreeSpec) -> Self {
        let mut commands = Vec::new();
        collect_command_specs(tree.commands, Vec::new(), &mut commands);
        Self { commands, tree }
    }

    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    pub fn tree(&self) -> &CommandTreeSpec {
        &self.tree
    }

    pub fn help(&self, version: &str) -> String {
        let mut help = format!(
            "harness {version}\n\nUSAGE:\n    harness <command> [options]\n\nGLOBAL OPTIONS:\n"
        );
        for option in self.tree.globals {
            help.push_str("    ");
            help.push_str(&option_usage(option, true));
            help.push('\n');
        }

        help.push_str("\nCOMMANDS:\n");
        for command in &self.commands {
            help.push_str("    ");
            help.push_str(&command.usage);
            help.push('\n');
        }

        if !self.tree.meta_commands.is_empty() {
            help.push_str("\nINTERACTIVE COMMANDS:\n");
            for command in self.tree.meta_commands {
                help.push_str("    ");
                help.push_str(command.name);
                if !command.aliases.is_empty() {
                    help.push_str(" (aliases: ");
                    help.push_str(&command.aliases.join(", "));
                    help.push(')');
                }
                help.push_str(" - ");
                help.push_str(command.about);
                help.push('\n');
            }
        }

        help
    }
}

pub fn build_cli() -> CommandCatalog {
    CommandCatalog::from_tree(phase2_command_tree_seed())
}

pub fn phase2_command_tree_seed() -> CommandTreeSpec {
    CommandTreeSpec {
        name: "harness",
        globals: &GLOBAL_OPTIONS,
        commands: &COMMANDS,
        meta_commands: &META_COMMANDS,
    }
}

const GLOBAL_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        long: "output",
        short: None,
        value_name: Some("human|json"),
        value: Some(ValueSpec {
            kind: ValueKind::OutputMode,
            source: ValueSource::Static(&["human", "json"]),
            help: "output format",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    },
    OptionSpec {
        long: "quiet",
        short: None,
        value_name: None,
        value: None,
        required: false,
        repeatable: false,
        action: OptionAction::SetTrue,
        requires: &[],
        conflicts_with: &[],
    },
    OptionSpec {
        long: "repo",
        short: None,
        value_name: Some("path"),
        value: Some(ValueSpec {
            kind: ValueKind::Path,
            source: ValueSource::FilesystemPath,
            help: "repository root",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    },
    OptionSpec {
        long: "state-dir",
        short: None,
        value_name: Some("path"),
        value: Some(ValueSpec {
            kind: ValueKind::Path,
            source: ValueSource::FilesystemPath,
            help: "state directory",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    },
];

const COMMANDS: &[CommandNodeSpec] = &[
    CommandNodeSpec {
        name: "init",
        about: "Initialize harness state in a repository.",
        aliases: &[],
        hidden: false,
        examples: &["harness init [--repo <path>]"],
        children: &[],
        positionals: &[],
        options: &[repo_option()],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "doctor",
        about: "Check harness configuration and provider readiness.",
        aliases: &[],
        hidden: false,
        examples: &["harness doctor [--offline] [--providers local|all] [--deep]"],
        children: &[],
        positionals: &[],
        options: &[
            flag_option("offline"),
            OptionSpec {
                long: "providers",
                short: None,
                value_name: Some("local|all"),
                value: Some(ValueSpec {
                    kind: ValueKind::ProviderScope,
                    source: ValueSource::Static(&["local", "all"]),
                    help: "provider checks to run",
                }),
                required: false,
                repeatable: false,
                action: OptionAction::Set,
                requires: &[],
                conflicts_with: &[],
            },
            flag_option("deep"),
        ],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "completions",
        about: "Generate shell completion script.",
        aliases: &[],
        hidden: false,
        examples: &["harness completions <bash|zsh|fish>"],
        children: &[],
        positionals: &[PositionalSpec {
            name: "shell",
            value: ValueSpec {
                kind: ValueKind::Shell,
                source: ValueSource::Static(&["bash", "zsh", "fish"]),
                help: "shell completion format",
            },
            required: true,
            required_unless_present: &[],
            repeatable: false,
            conflicts_with: &[],
        }],
        options: &[],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "task",
        about: "Manage tasks.",
        aliases: &[],
        hidden: false,
        examples: &[],
        children: &[
            CommandNodeSpec {
                name: "create",
                about: "Create a task.",
                aliases: &[],
                hidden: false,
                examples: &[
                    "harness task create --title <title> --goal <goal> --validation <cmd>...",
                ],
                children: &[],
                positionals: &[],
                options: &CREATE_TASK_OPTIONS,
                action: CommandAction::Dispatch,
            },
            CommandNodeSpec {
                name: "list",
                about: "List tasks.",
                aliases: &[],
                hidden: false,
                examples: &["harness task list [--status <status>]"],
                children: &[],
                positionals: &[],
                options: &[task_status_option()],
                action: CommandAction::Dispatch,
            },
            CommandNodeSpec {
                name: "get",
                about: "Show a task.",
                aliases: &[],
                hidden: false,
                examples: &["harness task get <task-id>"],
                children: &[],
                positionals: &[task_id_positional(true)],
                options: &[],
                action: CommandAction::Dispatch,
            },
            CommandNodeSpec {
                name: "run",
                about: "Run a task once.",
                aliases: &[],
                hidden: false,
                examples: &[
                    "harness task run <task-id> [--max-attempts <n>] [--model <ollama-model>]",
                ],
                children: &[],
                positionals: &[task_id_positional(true)],
                options: &[
                    max_attempts_option(),
                    model_option("ollama-model", "local coding model"),
                ],
                action: CommandAction::Dispatch,
            },
            CommandNodeSpec {
                name: "cleanup",
                about: "Clean up task workspace resources.",
                aliases: &[],
                hidden: false,
                examples: &["harness task cleanup <task-id> [--force] [--dry-run]"],
                children: &[],
                positionals: &[task_id_positional(true)],
                options: &[flag_option("force"), flag_option("dry-run")],
                action: CommandAction::Placeholder,
            },
        ],
        positionals: &[],
        options: &[],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "ticket",
        about: "Manage escalation tickets.",
        aliases: &[],
        hidden: false,
        examples: &[],
        children: &[
            CommandNodeSpec {
                name: "list",
                about: "List tickets.",
                aliases: &[],
                hidden: false,
                examples: &["harness ticket list [--status <status>]"],
                children: &[],
                positionals: &[],
                options: &[ticket_status_option()],
                action: CommandAction::Dispatch,
            },
            CommandNodeSpec {
                name: "get",
                about: "Show a ticket.",
                aliases: &[],
                hidden: false,
                examples: &["harness ticket get <ticket-id>"],
                children: &[],
                positionals: &[ticket_id_positional(true)],
                options: &[],
                action: CommandAction::Dispatch,
            },
            CommandNodeSpec {
                name: "resolve",
                about: "Resolve a ticket with the ticket provider.",
                aliases: &[],
                hidden: false,
                examples: &["harness ticket resolve <ticket-id> [--model <openai-model>]"],
                children: &[],
                positionals: &[ticket_id_positional(true)],
                options: &[model_option("openai-model", "ticket resolution model")],
                action: CommandAction::Dispatch,
            },
        ],
        positionals: &[],
        options: &[],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "resume",
        about: "Resume a task after ticket resolution.",
        aliases: &[],
        hidden: false,
        examples: &[
            "harness resume <task-id> [--ticket <ticket-id>] [--max-attempts <n>] [--model <ollama-model>]",
        ],
        children: &[],
        positionals: &[task_id_positional(true)],
        options: &[
            ticket_scoped_option(),
            max_attempts_option(),
            model_option("ollama-model", "local coding model"),
        ],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "run",
        about: "Create and run a task once.",
        aliases: &[],
        hidden: false,
        examples: &[
            "harness run --title <title> --goal <goal> --validation <cmd>... [--max-attempts <n>] [--model <model>]",
        ],
        children: &[],
        positionals: &[],
        options: &[
            title_option(true),
            goal_option(true),
            validation_option(true),
            max_attempts_option(),
            model_option("model", "local coding model"),
        ],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "config",
        about: "Inspect or update configuration.",
        aliases: &[],
        hidden: false,
        examples: &[],
        children: &[
            CommandNodeSpec {
                name: "get",
                about: "Show configuration.",
                aliases: &[],
                hidden: false,
                examples: &["harness config get"],
                children: &[],
                positionals: &[],
                options: &[],
                action: CommandAction::Placeholder,
            },
            CommandNodeSpec {
                name: "set",
                about: "Set a configuration value.",
                aliases: &[],
                hidden: false,
                examples: &["harness config set <key> <value>"],
                children: &[],
                positionals: &[
                    free_text_positional("key", true),
                    free_text_positional("value", true),
                ],
                options: &[],
                action: CommandAction::Placeholder,
            },
        ],
        positionals: &[],
        options: &[],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "workspace",
        about: "Manage workspaces.",
        aliases: &[],
        hidden: false,
        examples: &[],
        children: &[CommandNodeSpec {
            name: "prune",
            about: "Prune old workspace resources.",
            aliases: &[],
            hidden: false,
            examples: &["harness workspace prune [--dry-run] [--force]"],
            children: &[],
            positionals: &[],
            options: &[flag_option("dry-run"), flag_option("force")],
            action: CommandAction::Placeholder,
        }],
        positionals: &[],
        options: &[],
        action: CommandAction::Dispatch,
    },
    CommandNodeSpec {
        name: "supervise",
        about: "Run the foreground supervisor for an existing or newly-created task.",
        aliases: &[],
        hidden: false,
        examples: &[
            "harness supervise <task-id> [--ticket <ticket-id>] [--max-attempts <n>] [--model <ollama-model>] [--ticket-model <openai-model>] [--max-cycles <n>]",
            "harness supervise --create --title <title> --goal <goal> --validation <cmd>... [--max-attempts <n>] [--model <ollama-model>] [--ticket-model <openai-model>] [--max-cycles <n>]",
        ],
        children: &[],
        positionals: &[task_id_positional(false)],
        options: &[
            OptionSpec {
                long: "create",
                short: None,
                value_name: None,
                value: None,
                required: false,
                repeatable: false,
                action: OptionAction::SetTrue,
                requires: &["title", "goal", "validation"],
                conflicts_with: &["task-id", "ticket"],
            },
            create_field_option(title_option(false)),
            create_field_option(goal_option(false)),
            create_field_option(validation_option(false)),
            create_ticket_scoped_option(),
            max_attempts_option(),
            model_option("ollama-model", "local coding model"),
            ticket_model_option(),
            max_cycles_option(),
        ],
        action: CommandAction::Placeholder,
    },
];

const CREATE_TASK_OPTIONS: &[OptionSpec] = &[
    title_option(true),
    goal_option(true),
    validation_option(true),
];

const META_COMMANDS: &[MetaCommandSpec] = &[
    MetaCommandSpec {
        name: "exit",
        about: "Exit the interactive UI.",
        aliases: &[],
        action: MetaCommandAction::Exit,
    },
    MetaCommandSpec {
        name: "quit",
        about: "Exit the interactive UI.",
        aliases: &[],
        action: MetaCommandAction::Exit,
    },
    MetaCommandSpec {
        name: "help",
        about: "Show help.",
        aliases: &["?"],
        action: MetaCommandAction::Help,
    },
];

const fn flag_option(long: &'static str) -> OptionSpec {
    OptionSpec {
        long,
        short: None,
        value_name: None,
        value: None,
        required: false,
        repeatable: false,
        action: OptionAction::SetTrue,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn title_option(required: bool) -> OptionSpec {
    OptionSpec {
        long: "title",
        short: None,
        value_name: Some("title"),
        value: Some(ValueSpec {
            kind: ValueKind::FreeText,
            source: ValueSource::NoCompletion,
            help: "created task title",
        }),
        required,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn goal_option(required: bool) -> OptionSpec {
    OptionSpec {
        long: "goal",
        short: None,
        value_name: Some("goal"),
        value: Some(ValueSpec {
            kind: ValueKind::FreeText,
            source: ValueSource::NoCompletion,
            help: "created task goal",
        }),
        required,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn validation_option(required: bool) -> OptionSpec {
    OptionSpec {
        long: "validation",
        short: None,
        value_name: Some("cmd"),
        value: Some(ValueSpec {
            kind: ValueKind::FreeText,
            source: ValueSource::NoCompletion,
            help: "validation command",
        }),
        required,
        repeatable: true,
        action: OptionAction::Append,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn repo_option() -> OptionSpec {
    OptionSpec {
        long: "repo",
        short: None,
        value_name: Some("path"),
        value: Some(ValueSpec {
            kind: ValueKind::Path,
            source: ValueSource::FilesystemPath,
            help: "repository root",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn task_status_option() -> OptionSpec {
    OptionSpec {
        long: "status",
        short: None,
        value_name: Some("status"),
        value: Some(ValueSpec {
            kind: ValueKind::TaskStatus,
            source: ValueSource::Static(&["ready", "running", "stuck", "complete", "failed"]),
            help: "task status",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn ticket_status_option() -> OptionSpec {
    OptionSpec {
        long: "status",
        short: None,
        value_name: Some("status"),
        value: Some(ValueSpec {
            kind: ValueKind::TicketStatus,
            source: ValueSource::Static(&["open", "resolving", "resolved", "failed"]),
            help: "ticket status",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn task_id_positional(required: bool) -> PositionalSpec {
    PositionalSpec {
        name: "task-id",
        value: ValueSpec {
            kind: ValueKind::TaskId,
            source: ValueSource::StateQuery(StateQueryKind::TaskId {
                statuses: &["ready", "stuck", "running", "complete", "failed"],
            }),
            help: "task id",
        },
        required,
        required_unless_present: if required { &[] } else { &["create"] },
        repeatable: false,
        conflicts_with: if required { &[] } else { &["create"] },
    }
}

const fn ticket_id_positional(required: bool) -> PositionalSpec {
    PositionalSpec {
        name: "ticket-id",
        value: ValueSpec {
            kind: ValueKind::TicketId,
            source: ValueSource::StateQuery(StateQueryKind::TicketId {
                statuses: &["open", "resolved", "failed"],
                scoped_to_task_arg: false,
            }),
            help: "ticket id",
        },
        required,
        required_unless_present: &[],
        repeatable: false,
        conflicts_with: &[],
    }
}

const fn free_text_positional(name: &'static str, required: bool) -> PositionalSpec {
    PositionalSpec {
        name,
        value: ValueSpec {
            kind: ValueKind::FreeText,
            source: ValueSource::NoCompletion,
            help: "value",
        },
        required,
        required_unless_present: &[],
        repeatable: false,
        conflicts_with: &[],
    }
}

const fn create_field_option(mut option: OptionSpec) -> OptionSpec {
    option.requires = &["create"];
    option
}

const fn create_ticket_scoped_option() -> OptionSpec {
    let mut option = ticket_scoped_option();
    option.conflicts_with = &["create"];
    option
}

const fn ticket_scoped_option() -> OptionSpec {
    OptionSpec {
        long: "ticket",
        short: None,
        value_name: Some("ticket-id"),
        value: Some(ValueSpec {
            kind: ValueKind::TicketId,
            source: ValueSource::StateQuery(StateQueryKind::TicketId {
                statuses: &["open", "resolved", "failed"],
                scoped_to_task_arg: true,
            }),
            help: "ticket to resolve or resume from",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn max_attempts_option() -> OptionSpec {
    positive_integer_option("max-attempts", "n", "local run attempts")
}

const fn max_cycles_option() -> OptionSpec {
    positive_integer_option("max-cycles", "n", "supervisor escalation cycle cap")
}

const fn positive_integer_option(
    long: &'static str,
    value_name: &'static str,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        long,
        short: None,
        value_name: Some(value_name),
        value: Some(ValueSpec {
            kind: ValueKind::PositiveInteger,
            source: ValueSource::NoCompletion,
            help,
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn model_option(value_name: &'static str, help: &'static str) -> OptionSpec {
    OptionSpec {
        long: "model",
        short: None,
        value_name: Some(value_name),
        value: Some(ValueSpec {
            kind: ValueKind::Model,
            source: ValueSource::HintOnly,
            help,
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

const fn ticket_model_option() -> OptionSpec {
    OptionSpec {
        long: "ticket-model",
        short: None,
        value_name: Some("openai-model"),
        value: Some(ValueSpec {
            kind: ValueKind::Model,
            source: ValueSource::HintOnly,
            help: "ticket resolution model",
        }),
        required: false,
        repeatable: false,
        action: OptionAction::Set,
        requires: &[],
        conflicts_with: &[],
    }
}

fn collect_command_specs(
    nodes: &'static [CommandNodeSpec],
    path: Vec<&'static str>,
    commands: &mut Vec<CommandSpec>,
) {
    for node in nodes {
        let mut next_path = path.clone();
        next_path.push(node.name);
        if node.children.is_empty() {
            let usages = if node.examples.is_empty() {
                vec![generated_usage(&next_path, node)]
            } else {
                node.examples
                    .iter()
                    .map(|example| strip_binary_name(example).to_string())
                    .collect()
            };
            for usage in usages {
                commands.push(CommandSpec {
                    path: next_path.clone(),
                    usage,
                });
            }
        } else {
            collect_command_specs(node.children, next_path, commands);
        }
    }
}

fn generated_usage(path: &[&'static str], node: &CommandNodeSpec) -> String {
    let mut usage = path.join(" ");
    for positional in node.positionals {
        usage.push(' ');
        if positional.required {
            usage.push('<');
            usage.push_str(positional.name);
            usage.push('>');
        } else {
            usage.push_str("[<");
            usage.push_str(positional.name);
            usage.push_str(">]");
        }
    }
    for option in node.options {
        usage.push(' ');
        usage.push_str(&option_usage(option, false));
    }
    usage
}

fn option_usage(option: &OptionSpec, force_plain: bool) -> String {
    let mut usage = String::new();
    if !option.required && !force_plain {
        usage.push('[');
    }
    usage.push_str("--");
    usage.push_str(option.long);
    if let Some(value_name) = option.value_name {
        usage.push_str(" <");
        usage.push_str(value_name);
        usage.push('>');
        if option.repeatable {
            usage.push_str("...");
        }
    }
    if !option.required && !force_plain {
        usage.push(']');
    }
    usage
}

fn strip_binary_name(example: &str) -> &str {
    example.strip_prefix("harness ").unwrap_or(example)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_catalog_command_metadata_smoke_test() {
        let tree = phase2_command_tree_seed();

        assert_eq!(tree.name, "harness");
        assert!(tree.globals.iter().any(|option| option.long == "output"));
        assert!(
            tree.meta_commands
                .iter()
                .any(|command| command.name == "exit")
        );

        let supervise = tree
            .commands
            .iter()
            .find(|command| command.name == "supervise")
            .expect("supervise command seed");
        assert_eq!(supervise.action, CommandAction::Placeholder);
        assert!(supervise.positionals.iter().any(|arg| {
            arg.name == "task-id" && arg.value.kind == ValueKind::TaskId && !arg.required
        }));
        assert!(supervise.options.iter().any(|option| {
            option.long == "validation"
                && option.repeatable
                && option.action == OptionAction::Append
        }));
        assert!(supervise.options.iter().any(|option| {
            option.long == "ticket-model"
                && option
                    .value
                    .as_ref()
                    .is_some_and(|value| value.kind == ValueKind::Model)
        }));
    }
}
