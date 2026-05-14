# Harness Supervisor

`harness` is a CLI and terminal UI for running goal-driven coding tasks against a git repository. It creates isolated task worktrees, asks a local Ollama-compatible coding model for patches, validates those patches with configured commands, and automatically escalates stuck runs into tickets that can be resolved through an OpenAI-compatible provider.

Use a disposable or clean git repository first. The tool creates `.harness/` state, logs, artifacts, and task worktrees, and it applies model-generated patches inside task worktrees.

## Build

From this repository:

```sh
cargo build
./target/debug/harness --help
```

For day-to-day use, install the local binary so examples can use `harness` directly:

```sh
cargo install --path .
harness --help
```

You can also run through Cargo during development:

```sh
cargo run -- --help
```

## Prerequisites

- Rust and Cargo installed.
- A target git repository with a committed baseline.
- A local Ollama-compatible model server for task execution.
- Optional but recommended: `ARM_OPENAI_API_KEY` or `OPENAI_API_KEY` for ticket resolution.

Default provider settings are written to `.harness/config.toml` during `init`:

- Ollama base URL: `http://localhost:11434`
- Ollama model: `maternion/strand-rust-coder:latest`
- OpenAI-compatible base URL: Arm OpenAI API proxy
- OpenAI-compatible model: `gpt-5.3-codex`
- API key env: `ARM_OPENAI_API_KEY`, fallback `OPENAI_API_KEY`

Edit `.harness/config.toml` in the target repo if your provider setup differs.

## Quick Start

Set a target repository:

```sh
REPO=/path/to/your/git/repo
```

Initialize harness state and config:

```sh
harness --repo "$REPO" init --output json
```

Check local setup without provider network calls:

```sh
harness --repo "$REPO" doctor --offline --output json
```

Check local Ollama readiness:

```sh
harness --repo "$REPO" doctor --providers local --output json
```

Run a complete supervised task in one command:

```sh
harness --repo "$REPO" supervise \
  --create \
  --title "Fix failing tests" \
  --goal "Make the test suite pass with the smallest safe patch" \
  --validation "cargo test" \
  --max-attempts 2 \
  --max-cycles 3 \
  --output json
```

This command creates a task, runs local Ollama attempts, creates a ticket if the task gets stuck, resolves the ticket with the OpenAI-compatible provider, resumes local work with that advisory resolution, and repeats until the task completes or the cycle limit is reached.

## Interactive TUI

Run `harness` with no command to open the terminal UI:

```sh
harness --repo "$REPO"
```

The TUI is the main manual operating mode. It includes:

- A status header with repo/model/task/ticket context.
- A transcript of command output, supervisor progress, validation summaries, and shell escape output.
- Task and ticket panes for inspecting current state.
- A bottom prompt with command highlighting, contextual suggestions, and completion.

Prompt controls:

- `Tab`: complete commands, flags, static values, task IDs, and ticket IDs.
- `Up` / `Down`: move through suggestions or command history.
- `Enter`: apply the selected suggestion, or run the command when complete.
- `Ctrl-A` / `Ctrl-E`: move to start/end of the prompt.
- `Ctrl-U`: clear before cursor.
- `Ctrl-W`: delete the previous word.
- `Ctrl-C`: clear the current line; during foreground `supervise`, request cooperative cancellation.
- `Ctrl-D`, `exit`, or `quit`: leave the TUI.
- `!<command>`: run a shell escape from the repo root with a sanitized environment.

Inside the TUI, type commands without the leading `harness`:

```text
> task list
> supervise --create --title "Fix parser" --goal "Make cargo test pass" --validation "cargo test"
> ticket list --status open
> !git status --short
```

When stdin/stdout are not TTYs, no-command mode falls back to a simple line-oriented shell for tests and piped input instead of rendering the full TUI.

## Supervised Execution

The recommended automated path is `supervise`. It replaces the old manual loop of `task run -> ticket resolve -> resume`.

Create and supervise a new task:

```sh
harness --repo "$REPO" supervise \
  --create \
  --title "Fix failing tests" \
  --goal "Make cargo test pass with the smallest safe patch" \
  --validation "cargo test" \
  --max-attempts 2 \
  --max-cycles 3 \
  --output json
```

Supervise an existing task:

```sh
harness --repo "$REPO" supervise <task-id> \
  --max-attempts 2 \
  --max-cycles 3 \
  --output json
```

Resume from a specific resolved ticket:

```sh
harness --repo "$REPO" supervise <task-id> \
  --ticket <ticket-id> \
  --max-attempts 1 \
  --output json
```

Useful options:

- `--model <ollama-model>`: override the local coding model.
- `--ticket-model <openai-model>`: override the ticket-resolution model.
- `--max-attempts <n>`: local coding attempts per run/resume.
- `--max-cycles <n>`: maximum stuck-ticket-resolution-resume cycles.
- `--output json`: emit machine-readable command output and progress events.
- `--quiet`: suppress human-oriented informational output.

`supervise` is a foreground command, not a daemon. If it stops because of cancellation, provider failure, or a cycle limit, state is persisted in `.harness/`; rerun `supervise <task-id>` or use the lower-level commands below to inspect and continue.

## Lower-Level Commands

Create a task without running it:

```sh
harness --repo "$REPO" task create \
  --title "Fix failing tests" \
  --goal "Make the test suite pass with the smallest safe patch" \
  --validation "cargo test" \
  --output json
```

Run a task once:

```sh
harness --repo "$REPO" task run <task-id> \
  --max-attempts 2 \
  --output json
```

If a run completes, it exits with code `0`. If it gets stuck, it exits with code `10` and returns a `ticket_id`.

Resolve and resume manually:

```sh
export ARM_OPENAI_API_KEY=...

harness --repo "$REPO" ticket resolve <ticket-id> --output json
harness --repo "$REPO" resume <task-id> --ticket <ticket-id> --max-attempts 1 --output json
```

Inspect state:

```sh
harness --repo "$REPO" task list
harness --repo "$REPO" task list --status ready
harness --repo "$REPO" task get <task-id>
harness --repo "$REPO" ticket list
harness --repo "$REPO" ticket list --status open
harness --repo "$REPO" ticket get <ticket-id>
```

One-shot create-and-run without automatic ticket resolution:

```sh
harness --repo "$REPO" run \
  --title "Fix failing tests" \
  --goal "Make cargo test pass" \
  --validation "cargo test" \
  --max-attempts 2 \
  --output json
```

## Shell Completions

Interactive TUI completion works automatically inside `harness`.

For your outer shell, generate completion scripts with:

```sh
harness completions bash
harness completions zsh
harness completions fish
```

Example install commands:

```sh
# Bash
mkdir -p ~/.local/share/bash-completion/completions
harness completions bash > ~/.local/share/bash-completion/completions/harness

# Zsh
mkdir -p ~/.zfunc
harness completions zsh > ~/.zfunc/_harness
# Ensure ~/.zfunc is in fpath from your zsh config.

# Fish
mkdir -p ~/.config/fish/completions
harness completions fish > ~/.config/fish/completions/harness.fish
```

Open a new shell after installing completions.

## Exit Codes

- `0`: command completed.
- `1`: command failed.
- `2`: usage or parse error.
- `10`: task is stuck and needs a ticket resolution.
- `20`: doctor/readiness failure.
- `30`: security policy blocked the operation.

## Files Created

In the target repository:

- `.harness/config.toml`: provider and runtime configuration.
- `.harness/state.sqlite`: task, run, ticket, artifact, and event state.
- `.harness/artifacts/`: prompts, responses, patches, validation logs, manifests, and ticket artifacts.
- `.harness/logs/`: runtime logs.

Task worktrees are created under the configured worktree root from `.harness/config.toml`.

## Safety Notes

- Provider prompts, artifacts, stdout/stderr, and SQLite fields are redacted for obvious secrets before persistence/output.
- OpenAI-compatible ticket resolution is advisory only. Its output is stored as ticket-resolution evidence and is never directly applied as a patch.
- Ticket resolutions are consumed only after they are included in the next local Ollama prompt.
- Patch safety and workspace isolation still gate model-generated changes.
- CI-style e2e tests use fake providers; real provider smoke tests should be run manually against disposable repos.
