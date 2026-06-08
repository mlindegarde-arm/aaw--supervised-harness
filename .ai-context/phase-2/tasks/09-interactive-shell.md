# Workstream 9: Interactive Shell

## Scope

Implement the minimal line-editor interactive shell over `CommandRuntime`, including in-memory history, meta-commands, shell escapes, interrupt behavior, and interactive output sink wiring.

## Owned Files

- [x] `src/interactive/**`

## Shared Files

- [x] `src/cli/mod.rs` for no-subcommand startup wiring (`src/main.rs` unchanged).

## Interfaces Consumed

- [x] CLI command catalog and `CommandRuntime` from Workstream 1.
- [x] Security environment sanitizer from Workstream 6.
- [x] Workspace shell escape runner from Workstream 4.

## Interfaces Produced

- [x] Interactive shell entrypoint.
- [x] `InteractiveSink`.
- [x] Meta-command handling for `exit` and `quit`.
- [x] Shell escape dispatch for `!<command>`.

## Tests To Add

- [x] Tests for no-subcommand mode selecting interactive shell.
- [x] Tests for optional leading `harness` stripping in interactive input.
- [x] Tests that invalid commands do not exit the shell.
- [x] Tests for `exit`, `quit`, Ctrl-D, and empty input behavior where feasible.
- [x] Tests for shell escape dispatch and empty `!` usage error.
- [x] Tests that obvious secret assignments are not recorded in history.

## Acceptance Command

- [x] `cargo test interactive`
- [x] `cargo test`

## Blocked By

- [x] Workstream 1.
- [x] Workstream 6.

## Implementation Checklist

- [x] Use a line-editor abstraction such as `rustyline` or `reedline`.
- [x] Start interactive shell when `harness` has no subcommand.
- [x] Maintain in-memory history only.
- [x] Support Up/Down history recall through the selected line editor.
- [x] Make Ctrl-D exit.
- [x] Make Ctrl-C clear current line or interrupt active command where supported.
- [x] Prevent invalid commands from exiting the shell.
- [x] Dispatch normal input through `CommandRuntime`.
- [x] Handle `exit` and `quit` before `clap`.
- [x] Run shell escapes from repo root through sanitized environment.
- [x] Keep the shell open regardless of shell escape exit code.
- [x] Defer tab completion for MVP but preserve command catalog introspection.

## Review Checklist

- [x] Confirm interactive mode and one-shot mode share the same runtime.
- [x] Confirm shell escapes do not become task attempts.
- [x] Confirm secret-looking commands are not persisted in history.
- [x] Confirm shell escape output flows through `InteractiveSink`.
- [x] Confirm Ctrl-D/C behavior is documented by tests or clear implementation boundaries.

## Interactive Review Remediation

- [x] Replaced production stdin-only prompting with `rustyline` terminal input when stdin is a TTY.
- [x] Kept injectable `BufReadLineEditor` for tests and piped input fallback.
- [x] Routed shell escape stdout, stderr, and status diagnostics through `InteractiveSink` writer methods.
- [x] Added regression coverage for terminal-editor history tracking and shell escape output/status handling.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Minimal shell supports command dispatch and shell escapes.
- [x] Reviewer has passed this workstream.
