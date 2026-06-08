# Workstream C1: TUI State, Layout, and Rendering

Design reference: `.ai-context/phase-2-design.md` sections `TUI Design`, `Rendering Requirements`, `Pane Interaction Contract`, and `Transcript and Output Sanitization`.

## Scope

- [x] Build pure TUI state/rendering models using `ratatui`.
- [x] Implement transcript and side-pane models.
- [x] Centralize redaction and terminal-control sanitization for TUI text.

## Owned Files

- [x] `src/tui/app_state.rs`
- [x] `src/tui/render.rs`
- [x] `src/tui/panes.rs`
- [x] `src/tui/transcript.rs`
- [x] `src/tui/theme.rs`
- [x] TUI render/model tests

## Shared Files

- [x] Coordinate `src/tui/mod.rs` exports with C2/C3.
- [x] Do not implement runtime command execution; C3 owns that.

## Blocked By

- [x] Workstream 0 TUI contracts available.

## Interfaces Consumed

- [x] `TranscriptEvent`
- [x] `PaneStateSnapshot`
- [x] Redactor/sanitizer APIs.

## Interfaces Produced

- [x] TUI state model.
- [x] Responsive layout functions.
- [x] Transcript append/render boundary.
- [x] Pane models with focus/selection/scroll state.

## Implementation Checklist

- [x] Add TUI module skeleton if absent.
- [x] Implement status header, footer, transcript, and side-pane state.
- [x] Implement switchable panes for tasks, tickets, runs, and artifacts/logs.
- [x] Implement empty/loading/error states for panes.
- [x] Implement pane focus, selection, scrolling, and refresh markers.
- [x] Implement narrow/wide responsive layout behavior.
- [x] Implement transcript scrollback for the current session.
- [x] Implement `append_untrusted_text` with redaction, ANSI/OSC/control stripping, line caps, byte caps, and truncation markers.
- [x] Add render/model tests without requiring a real terminal.
- [x] Run formatter. Targeted C1 files were formatted and checked with `rustfmt --edition 2024`; full `cargo fmt --check` is blocked by non-C1 formatting diffs in `src/completion/mod.rs`, `src/state/supervisor_queries.rs`, and `tests/e2e.rs`.
- [x] Run the acceptance command. The literal command is rejected by Cargo as invalid multi-filter syntax; ran `cargo test tui::render`, `cargo test tui::panes`, and `cargo test tui::transcript` separately after review remediation.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Narrow and wide layout tests.
- [x] Side-pane switching tests.
- [x] Transcript scrolling tests.
- [x] Pane empty/loading/error render tests.
- [x] Side-pane scroll rendering regression test.
- [x] Composer cursor-tail rendering regression test.
- [x] Redaction tests.
- [x] Terminal-control sequence sanitization tests, including OSC 52 and clear-screen controls.

## Acceptance Command

- [ ] `cargo test tui::render tui::panes tui::transcript`

Notes:

- Cargo rejects the literal command because it accepts only one test filter. Equivalent filters passed separately:
  - `cargo test tui::render`
  - `cargo test tui::panes`
  - `cargo test tui::transcript`
- 2026-05-14 C1 review remediation reran the equivalent filters above; all passed.

## Review Checklist

- [x] Reviewer verifies no untrusted text reaches the terminal without the sanitizer path.
- [x] Reviewer verifies rendering logic is testable without a live TTY.
- [x] Reviewer verifies C3 remains responsible for command execution.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review findings to remediate:

- [x] Side-pane scroll state is not rendered; `render_side_pane` ignores `panes.active_state().scroll`.
- [x] Composer rendering drops text after the cursor; `render_composer` renders only `text[..cursor]`.

Review notes:

- Initial review found two pure rendering defects.
- Remediation added side-pane scroll rendering and full composer-buffer rendering with regression tests.
- Fresh review subagent passed with no findings.
- Local verification passed: `cargo fmt --check`, `cargo test tui::render`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes. The literal command is invalid Cargo syntax, so the intended filters were run separately.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
