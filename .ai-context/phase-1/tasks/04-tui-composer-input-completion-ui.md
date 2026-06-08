# Workstream C2: Composer, Input, and Completion UI

Design reference: `.ai-context/phase-2-design.md` sections `Interactive TUI`, `Key Bindings`, `Prompt Reference From Volt PR 61`, and `TUI and Rustyline Integration`.

## Scope

- [x] Implement the Codex-like composer and raw key behavior.
- [x] Wire completion/readiness into the composer.
- [x] Preserve in-memory-only history with secret filtering.

## Owned Files

- [x] `src/tui/composer.rs`
- [x] `src/tui/input.rs`
- [x] Composer/input tests

## Shared Files

- [x] Coordinate `src/tui/mod.rs` exports with C1/C3.
- [x] Consume completion engine from Workstream B.
- [x] Do not implement runtime dispatch; C3 owns command execution.

## Blocked By

- [x] Workstream B completion engine stable.
- [x] Workstream A metadata stable.
- [x] Workstream C1 render contracts available.

## Interfaces Consumed

- [x] `CompletionEngine`
- [x] `CompletionSet`
- [x] `CommandReadiness`
- [x] TUI render state from C1.

## Interfaces Produced

- [x] Composer state and actions.
- [x] Key event to composer action translation.
- [x] Suggestion selection/application behavior.
- [x] History filtering behavior.

## Implementation Checklist

- [x] Inspect Volt PR 61 behavior summarized in `phase-2-design.md`.
- [x] Implement editable input buffer and cursor index.
- [x] Implement selected suggestion index and suggestion visibility state.
- [x] Implement syntax token classes for command, option, value, and error states.
- [x] Implement Tab completion for single suggestion, longest common prefix, and selected suggestion.
- [x] Implement Enter applying selected suggestion before execution.
- [x] Implement readiness hints when command is incomplete.
- [x] Implement Up/Down suggestion selection and fallback history navigation.
- [x] Implement Left/Right, Home/End, Backspace, Ctrl-A/E/U/W, Ctrl-C/D, and Esc behavior.
- [x] Implement shell-escape hint behavior.
- [x] Implement secret-looking command filtering for in-memory history.
- [x] Run formatter.
- [x] Run the acceptance command. The literal command is rejected by Cargo as invalid multi-filter syntax; equivalent filters passed separately.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Every documented key binding.
- [x] Suggestion insertion and replacement span behavior.
- [x] Longest common prefix behavior.
- [x] Enter readiness behavior.
- [x] Shell-escape hint behavior.
- [x] History navigation and secret filtering.
- [x] Scoped ID completion display compatibility.

## Acceptance Command

- [x] `cargo test tui::composer tui::input completion`

Notes:

- Cargo rejects the literal command because it accepts only one test filter.
- Equivalent filters passed:
  - `cargo test tui::composer`
  - `cargo test tui::input`
  - `cargo test completion` passed.
- Owned-file formatting check passed: `rustfmt --edition 2024 --check src/tui/composer.rs src/tui/input.rs src/tui/mod.rs`.

## Review Checklist

- [x] Reviewer verifies behavior matches the Phase 2 prompt contract.
- [x] Reviewer verifies no persistent history is introduced.
- [x] Reviewer verifies composer stays independent of runtime execution.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review findings to remediate:

- [x] Add documented `Home` / `End` bindings equivalent to `Ctrl-A` / `Ctrl-E`.
- [x] Make `Ctrl-D` exit only when the composer is empty; non-empty text should not request exit.

Remediation notes:

- `Home` and `End` are mapped to the same composer commands as `Ctrl-A` and `Ctrl-E`.
- `Ctrl-D` returns `ExitRequested` only for an empty composer; non-empty text returns `Noop` and leaves the buffer unchanged.
- Added regression coverage in `tui::input` and `tui::composer`.
- Independent re-review passed after remediation with no remaining findings.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes. The literal command is invalid Cargo syntax, so the intended filters were run separately.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
