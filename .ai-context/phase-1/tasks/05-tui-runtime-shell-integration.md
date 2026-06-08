# Workstream C3: TUI Runtime and Shell Integration

Design reference: `.ai-context/phase-2-design.md` sections `Event Loop`, `Shell Escape Contract`, and `TUI Design`.

## Scope

- [x] Route `harness` with no TTY subcommand to the full TUI.
- [x] Execute runtime commands through a worker thread/task and event channel.
- [x] Implement sanitized shell escapes and cooperative cancellation.

## Owned Files

- [x] `src/tui/app.rs`
- [x] `src/tui/runtime_bridge.rs`
- [x] `src/tui/shell_escape.rs`
- [x] `src/cli/mod.rs` only for TTY routing

## Shared Files

- [x] Coordinate `src/tui/mod.rs` exports with C1/C2.
- [x] Do not reimplement supervisor semantics; consume Workstream D events/results.

## Blocked By

- [x] Workstream C1 passed review.
- [x] Workstream C2 passed review.
- [x] Workstream D typed events stable enough to stream.

## Interfaces Consumed

- [x] `CommandRuntime`
- [x] TUI state/composer/render models.
- [x] `SuperviseProgressEvent`
- [x] Cancellation token contract.
- [x] `DefaultEnvironmentSanitizer`
- [x] Redaction/sanitization path from C1.

## Interfaces Produced

- [x] Live TUI app event loop.
- [x] Runtime bridge event channel.
- [x] Shell escape runner.
- [x] TTY routing from CLI.

## Implementation Checklist

- [x] Inspect current `cli` and `interactive` routing.
- [x] Add TTY detection that launches TUI for `harness` with no command.
- [x] Preserve non-TTY fallback behavior.
- [x] Implement app event loop with terminal raw mode and cleanup guards.
- [x] Execute foreground commands on a worker thread/task.
- [x] Disable prompt editing while foreground command runs but keep rendering and resize handling live.
- [x] Stream stdout/stderr/progress/finish/failure events into transcript.
- [x] Implement cooperative Ctrl-C cancellation.
- [x] Emit exact persisted resume commands after cancellation where available.
- [x] Implement `!` shell escapes with repo root cwd, sanitized env, null stdin, timeout, output cap, process-group cleanup, redacted output, and nonzero exit rendering.
- [x] Run formatter.
- [x] Run the acceptance command. Literal multi-filter form is rejected by Cargo; split filters were run.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Terminal cleanup on normal exit and error.
- [x] Runtime command event flow.
- [x] Prompt disabled/re-enabled around command execution.
- [x] Shell env sanitization for sensitive vars.
- [x] Shell escape redaction/control-sequence sanitization.
- [x] Ctrl-C cancellation path.
- [x] Ctrl-C cancellation of in-flight `!` shell escapes with prompt acknowledgement.
- [x] Runtime bridge live progress while supervise worker remains running.
- [x] TTY route vs non-TTY fallback route.

## Acceptance Command

- [x] `cargo test tui::app tui::shell_escape interactive` (Cargo rejected multiple test filters; `cargo test tui::app`, `cargo test tui::shell_escape`, `cargo test tui::runtime_bridge`, `cargo test interactive`, and the TTY route filter passed.)

## Remediation Notes

- [x] Review finding C3-1 remediated: TUI shell escapes now execute through a cancellation-aware runner, and Ctrl-C returns a `CancelAcknowledged` event promptly. The default runner owns the child process and terminates the process group on Unix-supported process-group execution.
- [x] Review finding C3-2 remediated: supervise commands now use a streaming service/runtime path so progress events are sent to the TUI bridge before the final command result.
- [x] Regression coverage added for a long-running shell escape cancellation path and for progress observed while a supervise worker is still running.
- [x] Remediation verification run: `cargo fmt --check`, `cargo test tui::app`, `cargo test tui::runtime_bridge`, `cargo test tui::shell_escape`, and `cargo test runtime::tests::json_sink`.
- [x] Independent re-review passed after remediation.
- [x] Full verification passed: `cargo test` with 241 unit tests, 14 e2e tests passing and 2 intentionally ignored, and 2 fixture tests.

## Review Checklist

- [x] Reviewer verifies terminal modes are restored.
- [x] Reviewer verifies shell escapes cannot leak sensitive environment variables.
- [x] Reviewer verifies command execution uses existing runtime/service path.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
