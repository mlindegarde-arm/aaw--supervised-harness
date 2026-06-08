# Next Steps

This file captures follow-up work that is intentionally outside the current MVP but should be prioritized next.

## 1. CLI and Interactive Autocomplete

### Goal

Add autocomplete for the interactive shell and, where practical, installable shell completions for the one-shot CLI.

### Scope

- Wire `rustyline` completion into the interactive shell.
- Complete top-level commands: `init`, `doctor`, `task`, `ticket`, `resume`, `run`, `config`, `workspace`, `version`, `help`, `exit`, and `quit`.
- Complete nested commands and flags from the existing runtime command catalog.
- Complete dynamic IDs from state where useful:
  - task IDs for `task get`, `task run`, `task cleanup`, and `resume`
  - ticket IDs for `ticket get`, `ticket resolve`, and `resume --ticket`
- Complete common flag values:
  - `--output human|json`
  - `--providers local|all`
  - known task/ticket statuses
- Preserve current behavior for invalid commands, Ctrl-C, Ctrl-D, and shell escapes.
- Keep secret-looking commands out of history.

### Acceptance Criteria

- Pressing Tab in interactive mode completes command names, subcommands, flags, and known IDs.
- Completion uses the same command definitions as the runtime parser, avoiding a separate stale command list.
- Dynamic completions read from the configured harness state for the selected repo.
- Autocomplete does not trigger provider calls or long-running operations.
- Tests cover command completion, flag completion, dynamic task/ticket ID completion, and shell escape non-completion.

## 2. Background Supervisor Loop

### Goal

Add a supervisor mode that automates the manual stuck-ticket workflow:

```text
task run -> stuck ticket -> ticket resolve -> resume -> repeat until complete or escalation limit
```

The supervisor should continue local worker progress through Ollama while using the OpenAI-compatible provider only for advisory ticket resolution.

### Candidate Command Shape

```sh
harness supervise <task-id> [--max-attempts <n>] [--model <ollama-model>] [--ticket-model <openai-model>]
```

Possible later convenience flag:

```sh
harness task run <task-id> --auto-resolve-tickets
```

### Scope

- Detect when `task run` or `resume` returns `stuck` with a `ticket_id`.
- Resolve open tickets automatically with the configured OpenAI-compatible provider.
- Resume the task with the resolved ticket using the configured local Ollama model.
- Repeat until:
  - task completes
  - max escalation cycles are reached
  - ticket resolution fails
  - security policy blocks evidence or provider input
  - no unconsumed resolved ticket is available
- Preserve existing safety semantics:
  - OpenAI output remains advisory only.
  - Ticket resolution is never directly applied as a patch.
  - Ticket resolutions are consumed only after inclusion in the next Ollama prompt.
  - Secrets are redacted before provider requests, output, artifacts, and persistence.
- Emit clear progress events in human and JSON output.
- Make the loop resumable after process interruption by relying on persisted task/ticket/run state.

### Acceptance Criteria

- A task that gets stuck can be completed with one `harness supervise <task-id>` command when fake providers supply a valid ticket resolution and follow-up patch.
- The supervisor exits `0` on completion.
- The supervisor exits `10` if the task remains stuck after allowed escalation cycles.
- The supervisor exits `20` for provider readiness failures.
- The supervisor exits `30` for security blocks.
- Tests cover:
  - success after one automatic ticket resolution
  - repeated stuck/resume cycles up to escalation limit
  - ticket resolution provider failure
  - resume provider failure without consuming the resolution
  - interruption/resume from persisted state
  - redaction across stdout, stderr, SQLite, artifacts, and provider requests

## Notes

- Autocomplete should be implemented before or alongside supervisor mode so long-running supervised workflows are easier to inspect and control interactively.
- Supervisor mode should not require a daemon for the first version. A foreground command is simpler and fits the existing CLI/runtime model.
- A true background daemon or scheduler can be considered later if foreground supervision proves useful.
