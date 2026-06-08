# Workstream F2: TUI PTY E2E

Design reference: `.ai-context/phase-2-design.md` sections `PTY Test Harness`, `Interactive TUI`, and `Rendering Requirements`.

## Scope

- [x] Add pseudo-terminal tests for the real TUI.
- [x] Use normalized virtual-screen assertions.
- [x] Cover prompt suggestions, shell escapes, side panes, and supervise streaming/cancellation.
  - Covered: prompt suggestion rendering/insertion, dynamic task ID completion in the TUI, shell escapes, supervise streaming/disabled composer, Ctrl-C cancellation during foreground supervise, side-pane switching, transcript scrolling, startup/exit, TTY/non-TTY route, invalid command recovery, env sanitization, narrow/wide smoke.

## Owned Files

- [x] `tests/tui_pty.rs` or `tests/e2e.rs`
- [x] `tests/support/pty.rs`

## Shared Files

- [x] Coordinate test support API changes with F0/F1.
- [x] Do not add raw sleep-based brittle tests.

## Blocked By

- [x] Workstream A passed review.
- [x] Workstream B passed review.
- [x] Workstream C1 passed review.
- [x] Workstream C2 passed review.
- [x] Workstream C3 passed review.
- [x] Workstream F0 passed review.

## Interfaces Consumed

- [x] Binary e2e support.
- [x] TUI app.
- [x] Completion engine.
- [x] Fake fixture state/provider support.

## Interfaces Produced

- [x] PTY runner.
- [x] Virtual screen normalizer.
- [x] TUI acceptance tests.

## Implementation Checklist

- [x] Add PTY process runner with fixed terminal size.
- [x] Add time-bounded polling for prompt/sentinel text.
- [x] Add ANSI parser/normalizer into virtual screen buffer.
- [x] Add terminal cleanup assertions after process exit.
- [x] Test TTY route opens TUI and non-TTY route uses fallback.
- [x] Test `task <Tab>` renders task subcommands.
- [x] Test seeded `resume task_<Tab>` renders task IDs with status/title context.
- [x] Test Down/Enter and Tab insert selected suggestions.
- [x] Test invalid command stays in UI and shows diagnostic.
- [x] Test `!env` proves shell environment sanitization.
- [x] Test shell escape output is redacted and terminal-control sanitized.
- [x] Test foreground `supervise` streams progress and disables composer input.
- [x] Test Ctrl-C during `supervise` requests cancellation and shows next command.
- [x] Test narrow and wide layouts do not overlap text.
- [x] Test side-pane switching and transcript scrolling.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Startup/exit smoke.
- [x] Prompt suggestions and dynamic ID completion.
- [x] Suggestion selection/insertion.
- [x] Invalid command recovery.
- [x] Shell env/redaction.
- [x] Supervise streaming/cancellation.
  - Streaming, disabled composer, and Ctrl-C cancellation covered.
- [x] Narrow/wide layout.
- [x] Side-pane and transcript navigation.

## Acceptance Command

- [x] `cargo test --test tui_pty`

## Review Checklist

- [x] Reviewer verifies PTY tests drive the actual binary.
- [x] Reviewer verifies assertions use virtual-screen normalization.
- [x] Reviewer verifies no raw sleeps are used for synchronization.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.

## Review Notes

- Independent review passed with no blocking findings.
- Full verification passed: `cargo test` with 241 unit tests, 21 e2e tests, 2 fixture tests, and 11 PTY tests.
