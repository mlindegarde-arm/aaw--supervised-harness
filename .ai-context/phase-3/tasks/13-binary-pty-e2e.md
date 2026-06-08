# Workstream J: Binary And PTY End-To-End Coverage

Design reference: `.ai-context/phase-3/Design.md`, Workstream J.

## Scope

Add end-to-end CLI and PTY coverage for the complete Phase 3 prompt-first objective workflow and preserve Phase 2 regressions.

## Owned Files

- `tests/e2e.rs`
- `tests/tui_pty.rs`
- `tests/support/**` scenario helpers only

## Shared Files

- None without coordination. This workstream should test public behavior rather than changing implementation modules.

## Blocked By

- Workstream C2.
- Workstream D.
- Workstream E.
- Workstream F.
- Workstream G2.
- Workstream H.
- Workstream I.

## Interfaces Consumed

- Binary CLI.
- TUI PTY harness.
- Fake planner/resolver/local provider infrastructure.
- Objective state store.

## Interfaces Produced

- Full CLI objective scenarios.
- Full PTY prompt-first scenarios.
- Phase 2 regression coverage.
- Failure matrix coverage.

## Implementation Checklist

- [x] Add CLI success scenario for `objective start --supervise --output json`.
- [x] Add CLI scenario for planner malformed output.
- [ ] Add CLI scenario for unsafe validation output.
- [x] Add CLI scenario for local stuck ticket resolver flow.
- [ ] Add CLI scenario for resolver malformed output.
- [ ] Add CLI scenario for validation repair success.
- [ ] Add CLI scenario for cancellation.
- [x] Add CLI scenario for max-cycle/blocking behavior.
- [x] Assert stdout is exactly one final JSON object for JSON e2e tests.
- [x] Assert every stderr line is an NDJSON event envelope for JSON e2e tests.
- [x] Add PTY scenario for plain prompt objective start.
- [x] Add PTY scenario for `/ta<Tab>` slash completion.
- [x] Add PTY scenario for dynamic task/ticket/objective completions.
- [x] Add PTY scenario proving plain `task list` shows compatibility warning.
- [x] Add PTY shell escape regression.
- [x] Add PTY lifecycle panel assertions.
- [x] Preserve Phase 2 CLI/TUI/shell completion regressions.

## Tests To Add

- [x] Binary objective success.
- [x] Binary planner rejection.
- [x] Binary ticket resolver flow.
- [ ] Binary validation repair.
- [ ] Binary cancellation.
- [x] Binary failure matrix cases.
- [x] PTY prompt-first flow.
- [x] PTY slash command flow.
- [x] PTY dashboard lifecycle flow.

## Acceptance Command

```sh
cargo test --test e2e --test tui_pty
```

## Review Checklist

- [x] Tests drive the actual CLI/TUI behavior, not only internal APIs.
- [x] Fake infrastructure makes e2e tests deterministic.
- [x] JSON contract assertions are strict.
- [x] Phase 2 regressions remain covered.

## Done Criteria

- [x] Acceptance command passes.
- [x] Phase 3 can be validated end-to-end from a user prompt to terminal objective state.
- [x] A separate review subagent passes this workstream.
