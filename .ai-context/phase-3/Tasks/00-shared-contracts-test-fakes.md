# Workstream 0: Shared Contracts And Test Fakes

Design reference: `.ai-context/phase-3/Design.md`, Workstream 0.

## Scope

Create the shared Phase 3 objective contracts that later workstreams consume: objective IDs/statuses, objective progress events, JSON/NDJSON event envelopes, fake provider contracts, fake clock, deterministic IDs, and provider call ledger support.

## Owned Files

- `src/domain/**`
- `src/runtime/events.rs`
- `tests/support/**`

## Shared Files

- `src/runtime/mod.rs` only if needed to expose event types.
- `src/lib.rs` or `src/main.rs` only if needed for module exports.

## Blocked By

- None.

## Interfaces Consumed

- Existing task, run, ticket, provider, and event domain types.
- Existing test support conventions.

## Interfaces Produced

- Objective ID newtypes and parse/wire-name behavior.
- `ObjectiveStatus`, acceptance status, validation review status, and planner exchange kind enums.
- `ObjectiveProgressEvent`.
- JSON/NDJSON `CommandEvent` envelope shape.
- Fake planner/resolver/local provider contracts.
- Provider call ledger interfaces.
- Fake clock and deterministic ID helpers.

## Implementation Checklist

- [x] Add objective ID newtypes with display, parse, serialization, and validation tests.
- [x] Add objective status enums with stable lowercase wire names.
- [x] Add acceptance, validation review, and planner exchange kind enums with stable wire names.
- [x] Add `ObjectiveProgressEvent` variants for the Phase 3 lifecycle.
- [x] Add a command event envelope that can represent objective events without breaking existing supervise events.
- [x] Add provider call ledger structs that can assert planner, resolver, and local worker call counts independently.
- [x] Add fake planner, fake resolver, and fake local implementation provider contracts without concrete Phase 3 behavior.
- [x] Add fake clock and deterministic ID helpers for objective tests.
- [x] Re-export only the contracts needed by later workstreams.

## Tests To Add

- [x] Objective ID parse/display round trips.
- [x] Objective status wire-name parsing and rejection tests.
- [x] Objective progress event serialization tests.
- [x] Command event envelope serialization tests.
- [x] Provider call ledger count/assertion tests.

## Acceptance Command

```sh
cargo test objective_id
cargo test objective_status
cargo test objective_progress_event
cargo test provider_call_ledger
```

## Review Checklist

- [ ] No persistence, CLI, TUI, planner, or monitor behavior is implemented here.
- [ ] Existing Phase 2 event serialization remains compatible.
- [ ] New contracts are stable enough for other workstreams to consume without editing this workstream.
- [ ] Tests are deterministic.

## Done Criteria

- [x] Acceptance command passes.
- [x] Later workstreams can consume the contracts without redefining objective/event/provider test types.
- [x] A separate review subagent passes this workstream.

## Implementation Notes

- 2026-05-14: Acceptance commands were split into separate `cargo test` invocations because Cargo accepts a single test-name filter per invocation.

## Review Findings

- 2026-05-14: Initial review failed because objective progress envelopes were not integrated into the `CommandEvent`/`JsonSink` runtime output path, the objective lifecycle contract lacked failed/resolving/validating coverage, and ID/status tests were too narrow.

## Remediation Notes

- 2026-05-14: Added objective progress support to `CommandEvent` and JSON output, added `ObjectiveProgressPhase` plus failed lifecycle coverage, expanded objective ID/status wire-contract tests, and reran the acceptance filters.

## Review Notes

- 2026-05-14: Fresh review subagent passed the remediated workstream. Reviewer verified prior findings were fixed and reported no blocking bugs or design mismatches. Full `cargo test` also passed.
