# Workstream C4: Rustyline Fallback Integration

Design reference: `.ai-context/phase-2-design.md` sections `Non-TTY Fallback` and `TUI and Rustyline Integration`.

## Scope

- [x] Keep non-TTY and fallback interactive behavior stable.
- [x] Use the shared completion engine where practical.
- [x] Avoid persistent history files.

## Owned Files

- [x] `src/interactive/fallback.rs`
- [x] Module export lines in `src/interactive/mod.rs`
- [x] Fallback interactive tests

## Shared Files

- [ ] Coordinate with C3 for routing expectations.
- [x] Do not implement full-screen TUI behavior here.

## Blocked By

- [x] Workstream B completion engine stable.

## Interfaces Consumed

- [x] `CompletionEngine`
- [x] `CompletionContext`
- [x] Existing `InteractiveShell` and `LineEditor` traits.

## Interfaces Produced

- [x] Rustyline helper if useful for TTY fallback.
- [x] Stable non-TTY line shell behavior.

## Implementation Checklist

- [x] Inspect current `src/interactive/mod.rs`.
- [x] Split fallback line-editor code into `src/interactive/fallback.rs` if needed.
- [x] Configure rustyline helper with shared completer where practical.
- [x] Add optional hinter/highlighter/validator only if low risk.
- [x] Preserve `BufReadLineEditor` behavior for tests and piped input.
- [x] Prove no `load_history`, `save_history`, or history-file creation occurs.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Non-TTY/piped behavior remains compatible.
- [x] Fallback completer delegates to completion engine where wired.
- [x] No persistent history is loaded/saved/created.

## Acceptance Command

- [x] `cargo test interactive`

## Review Checklist

- [x] Reviewer verifies no persistent history files are introduced.
- [x] Reviewer verifies fallback does not duplicate command metadata.
- [x] Reviewer verifies non-TTY behavior is stable.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review notes:

- Separate review subagent passed Workstream C4 with no findings.
- Local verification passed: `cargo fmt --check`, `cargo test interactive`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
