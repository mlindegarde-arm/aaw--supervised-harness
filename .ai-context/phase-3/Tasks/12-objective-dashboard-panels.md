# Workstream I: Objective Dashboard Panels

Design reference: `.ai-context/phase-3/Design.md`, Workstream I.

## Scope

Add objective-aware dashboard state and rendering so the TUI visually shows planning, running tasks, workers, open tickets, resolver activity, validation, and terminal states while preserving Phase 2 panels.

## Owned Files

- `src/tui/app_state.rs`
- `src/tui/panes.rs`
- `src/tui/render.rs`
- `src/tui/transcript.rs`

## Shared Files

- `src/runtime/events.rs` only through Workstream 0 contracts.
- Store/service snapshot APIs only through their public interfaces.

## Blocked By

- Workstream 0.
- Workstream B2.
- Workstream E.
- Workstream H contracts.

## Interfaces Consumed

- Objective events.
- Objective/task/ticket dashboard snapshots.
- TUI composer state.

## Interfaces Produced

- `DashboardPaneSnapshot`.
- Objective panel state.
- Objective event reducer.
- Visible lifecycle rendering.

## Implementation Checklist

- [x] Define dashboard snapshot model for objective, tasks, workers, tickets, validation, remote activity, and transcript.
- [x] Add objective event reducer.
- [x] Render objective summary/progress panel.
- [x] Render running generated tasks/local workers panel.
- [x] Render open tickets and resolver activity panel.
- [x] Render validation status panel.
- [x] Preserve Phase 2 panes when no objective is active.
- [x] Handle narrow terminal widths without text overlap.
- [x] Ensure terminal states are visually distinct.

## Tests To Add

- [x] Render tests for no objective.
- [x] Render tests for planning/running/ticket/resolver/validation/complete/blocked/cancelled states.
- [x] Event reducer tests.
- [x] Narrow-width layout tests.
- [x] Phase 2 pane regression tests.

## Acceptance Command

```sh
cargo test tui_dashboard
cargo test objective_panel
cargo test render
```

## Review Checklist

- [x] Panels make the harness visibly different from a plain chat client.
- [x] Layout does not overlap or truncate critical state.
- [x] Existing Phase 2 state remains visible.
- [x] Objective events and persisted snapshots converge on the same display state.

## Done Criteria

- [x] Acceptance command passes.
- [x] PTY tests can assert visible lifecycle states.
- [x] A separate review subagent passes this workstream.
