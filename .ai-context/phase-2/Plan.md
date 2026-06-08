# Rust Local Agent Orchestrator

## Summary

Build a greenfield Rust CLI/TUI that orchestrates coding work across local Ollama workers and OpenAI escalation. The v1 binary runs everything locally, but the internal architecture must keep orchestration, UI, state, workers, and model providers separated so it can later split into a daemon plus thin CLI/TUI without major refactoring.

The user experience should resemble Codex CLI: a full-screen terminal UI with a composer, command suggestions, status panes, task/ticket views, streaming logs, shell escapes, and background worker activity.

## Key Changes

- Create a single Rust binary, tentatively named `agent`, with these subsystems:
  - `cli`: `clap` command tree, command runtime, structured exit codes.
  - `tui`: `ratatui` + `crossterm` full-screen interface.
  - `state`: SQLite-backed task, worker, ticket, run, and event storage.
  - `orchestrator`: task leasing, worker scheduling, stuck detection, ticket lifecycle.
  - `providers`: Ollama provider and OpenAI Responses API provider.
  - `workspace`: git worktree management, diff capture, command execution, logs.
  - `repl`: composer input, completion, shell escapes, command dispatch.

- Use one local binary in v1, but define the orchestration API as a Rust trait boundary:
  - `OrchestratorService`
  - `TaskStore`
  - `ModelProvider`
  - `WorkspaceManager`
  - `CommandRunner`

- Implement Codex-like interactive behavior:
  - Running `agent` opens the TUI.
  - Running `agent <command>` executes a normal one-shot CLI command.
  - Running `agent --repl [command...]` opens interactive mode and optionally runs an initial command.
  - TUI commands include `plan`, `task list`, `task start`, `worker list`, `ticket list`, `ticket resolve`, `run`, `config`, `help`, `exit`, and shell escapes with `!`.

- Use git worktrees for task isolation:
  - Each task gets a leased worktree under `.agent/worktrees/<task-id>`.
  - Workers may only edit their assigned worktree.
  - The orchestrator records current diff, commands run, test output, attempts, and ticket references.
  - Completed tasks produce a final patch or merge-ready worktree.

- Implement the local RWL loop:
  - Read task instructions and current repo context.
  - Ask Ollama `maternion/strand-rust-coder:latest` for a small patch or next action.
  - The local Ollama endpoint has been verified at `http://localhost:11434` with model `maternion/strand-rust-coder:latest`.
  - The verified model reports family `qwen2`, parameter size `14.8B`, and quantization `Q4_K_M`.
  - Apply changes in the task worktree.
  - Run configured validation commands.
  - Feed failures back to the local model for bounded retries.
  - Mark stuck after configurable retry/failure thresholds and create a ticket.

- Implement OpenAI escalation through an OpenAI-compatible Responses API:
  - Tickets are resolved by the Responses API, not manual Codex CLI handoff.
  - The provider must accept a configurable `base_url` so it can target direct OpenAI or an internal proxy.
  - The default endpoint for this project is the ARM OpenAI proxy at `https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1`.
  - The verified project is `US-Services`; `openai-us` and the `openai` alias both authenticate through the proxy.
  - Ticket prompts include task intent, current diff, failing command, logs, prior attempts, and the precise unblock question.
  - Resolution may be an explanation, patch guidance, or direct patch depending on ticket type.
  - Resolved tickets are written back into SQLite and surfaced to the waiting worker.

## Interfaces

- Project state lives under `.agent/`:
  - `.agent/state.sqlite`
  - `.agent/config.toml`
  - `.agent/worktrees/`
  - `.agent/artifacts/`
  - `.agent/logs/`

- Core task fields:
  - `id`, `title`, `status`, `priority`, `created_at`, `updated_at`
  - `goal`, `acceptance_criteria`, `validation_commands`
  - `assigned_worker`, `worktree_path`, `attempt_count`

- Core ticket fields:
  - `id`, `task_id`, `status`, `created_at`, `resolved_at`
  - `blocked_on`, `question`, `evidence_paths`, `resolution`
  - `provider`, `model`, `response_id`

- Initial CLI command set:
  - `agent plan <goal>`
  - `agent task list|get|start|pause|resume`
  - `agent worker list|start|stop`
  - `agent ticket list|get|resolve`
  - `agent run`
  - `agent config get|set`
  - `agent tui`

- Initial config defaults:
  - Ollama base URL: `http://localhost:11434`
  - Ollama model: `maternion/strand-rust-coder:latest`
  - OpenAI-compatible base URL: `https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1`
  - OpenAI-compatible API key env var: `OPENAI_API_KEY`
  - OpenAI escalation model: `gpt-5.3-codex`
  - Max local repair attempts before ticket: `3`
  - Max concurrent Ollama workers: `1` for v1, designed to increase later.
  - Worker isolation: git worktrees.

- Example provider config:

```toml
[providers.ollama]
base_url = "http://localhost:11434"
default_model = "maternion/strand-rust-coder:latest"

[providers.openai]
base_url = "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-5.3-codex"
```

## TUI Behavior

- Layout:
  - Main transcript/activity pane.
  - Bottom composer with command suggestions.
  - Side or switchable pane for tasks, workers, tickets, and logs.
  - Status line showing active model, current workspace, worker count, open tickets, and running commands.

- Completion:
  - Suggest built-ins, subcommands, options, task IDs, ticket IDs, worker IDs, model names, and recent validation commands.
  - Use command-tree introspection for commands/options.
  - Use SQLite-backed recent entities for contextual values.
  - Support tab completion, arrow selection, and longest-common-prefix completion.

- Shell escapes:
  - `!<command>` runs a shell command from the current project root.
  - Sensitive environment variables are removed from shell escape commands by default.
  - Output streams into the transcript.

## Test Plan

- Unit tests:
  - CLI runtime can execute repeated commands without process exit.
  - Parser resolves command path, active fragment, options, and positional IDs.
  - Suggestion engine returns command, option, task, ticket, and model suggestions.
  - SQLite store preserves task/ticket state and lease transitions.
  - Worktree manager creates, leases, releases, and records diffs.
  - Ticket builder includes required evidence and omits missing/empty fields safely.
  - Shell escape environment sanitizer removes token/secret/password/API key variables.

- Integration tests:
  - `agent task start` creates a worktree and records a run.
  - A simulated Ollama worker succeeds after one patch.
  - The Ollama provider can list the configured model and perform a minimal non-streaming generation request.
  - A simulated Ollama worker fails repeatedly and creates a ticket.
  - A simulated OpenAI provider resolves a ticket and unblocks the task.
  - The OpenAI-compatible provider builds requests against the configured `base_url`, including proxy paths like `/api/providers/openai-us/v1/responses`.
  - TUI command runtime handles repeated command execution in one process.

- Manual acceptance:
  - Running `agent` opens the TUI.
  - Typing `task list`, `ticket list`, and `!git status` works.
  - Suggestions appear and can be accepted.
  - A sample task can move from planned, to running, to stuck, to resolved, to complete.

## Assumptions

- This is a new Rust project in the current empty directory.
- v1 is a single local binary, intentionally structured for a future daemon/CLI split.
- OpenAI escalation uses the API directly.
- The default escalation API is the ARM OpenAI proxy, not `https://api.openai.com/v1`.
- The ARM proxy has been verified for project `US-Services`, provider `openai-us`, model `gpt-5.3-codex`, and the Responses API.
- Ollama is installed and reachable locally at `http://localhost:11434`.
- The local model `maternion/strand-rust-coder:latest` has been verified for both model listing and generation.
- Git worktrees are the v1 isolation mechanism.
- SQLite is acceptable for local durable state.
- The first implementation should favor correctness and debuggability over high worker concurrency.
