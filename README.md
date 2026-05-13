# Harness Supervisor

`harness` is a CLI for running goal-driven coding tasks against a git repository. It creates isolated task worktrees, asks a local Ollama-compatible coding model for patches, validates those patches with configured commands, and creates tickets when a task gets stuck. Tickets can be resolved through an OpenAI-compatible provider and then used to resume the task.

Use a disposable or clean git repository first. The tool creates `.harness/` state, logs, artifacts, and task worktrees, and it applies model-generated patches inside task worktrees.

## Build

From this repository:

```sh
cargo build
./target/debug/harness --help
```

During development you can also run through Cargo:

```sh
cargo run -- --help
```

## Prerequisites

- Rust and Cargo installed.
- A target git repository with committed baseline work.
- A local Ollama-compatible model server for task execution.
- Optional: `ARM_OPENAI_API_KEY` or `OPENAI_API_KEY` for ticket resolution.

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
./target/debug/harness --repo "$REPO" init --output json
```

Check local setup without provider network calls:

```sh
./target/debug/harness --repo "$REPO" doctor --offline --output json
```

Check local Ollama readiness:

```sh
./target/debug/harness --repo "$REPO" doctor --providers local --output json
```

Create a task:

```sh
./target/debug/harness --repo "$REPO" task create \
  --title "Fix failing tests" \
  --goal "Make the test suite pass with the smallest safe patch" \
  --validation "cargo test" \
  --output json
```

Run the returned task:

```sh
./target/debug/harness --repo "$REPO" task run <task-id> \
  --max-attempts 2 \
  --output json
```

If the run completes, the task exits with code `0`. If it gets stuck, it exits with code `10` and returns a `ticket_id`.

Resolve a stuck ticket:

```sh
export ARM_OPENAI_API_KEY=...

./target/debug/harness --repo "$REPO" ticket resolve <ticket-id> --output json
```

Resume the task with the ticket resolution:

```sh
./target/debug/harness --repo "$REPO" resume <task-id> \
  --ticket <ticket-id> \
  --max-attempts 1 \
  --output json
```

## Common Commands

```sh
./target/debug/harness --repo "$REPO" task list
./target/debug/harness --repo "$REPO" task get <task-id>
./target/debug/harness --repo "$REPO" ticket list
./target/debug/harness --repo "$REPO" ticket get <ticket-id>
./target/debug/harness --repo "$REPO" task cleanup <task-id> --dry-run
./target/debug/harness --repo "$REPO" workspace prune --dry-run
```

Running with no subcommand starts the interactive shell:

```sh
./target/debug/harness --repo "$REPO"
```

Inside the shell:

- Run normal harness commands without the leading `harness`.
- Use `exit` or `quit` to leave.
- Prefix a command with `!` to run a shell escape from the repo root.

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

## Notes

- CI-style e2e tests use fake providers; real provider smoke tests should be run manually against disposable repos.
- Provider prompts, artifacts, stdout/stderr, and SQLite fields are redacted for obvious secrets before persistence/output.
- OpenAI-compatible ticket resolution is advisory only. Its output is stored as ticket-resolution evidence and is never directly applied as a patch.
