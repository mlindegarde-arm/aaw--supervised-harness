# Workstream E: Orchestrator/State Helpers

Design reference: `.ai-context/phase-2-design.md` sections `Ticket Selection`, `Running and Lease Recovery`, and `Supervisor Design`.

## Scope

- [x] Add supervisor-safe state selection helpers.
- [x] Keep ticket/run ordering stable and resumable.
- [x] Preserve existing lease semantics.

## Owned Files

- [x] `src/orchestrator/supervisor_state.rs`
- [x] `src/state/supervisor_queries.rs`
- [x] Minimal module wiring in `src/orchestrator/mod.rs`
- [x] Minimal module wiring in `src/state/mod.rs`

## Shared Files

- [x] Coordinate any trait shape changes with Workstream 0. No trait shape changes were needed.
- [x] Do not implement the supervisor loop; Workstream D owns loop semantics.

## Blocked By

- [x] Workstream 0 supervisor state-view/store contracts available.

## Interfaces Consumed

- [x] Task/ticket/run domain types.
- [x] Store connection/query APIs.
- [x] Supervisor state-view/store trait signatures.

## Interfaces Produced

- [x] Latest stuck run helper.
- [x] Latest ticket on latest stuck run helper.
- [x] Latest unconsumed resolved ticket helper.
- [x] Retryable resolving/failed ticket helper.
- [x] Persisted next-cycle calculation helper.

## Implementation Checklist

- [x] Inspect current SQLite schema and orchestrator ticket/run helpers.
- [x] Implement stable latest stuck run ordering by `started_at`, then run id.
- [x] Implement task-scoped ticket selection.
- [x] Reject stale `--ticket` values that do not belong to the latest stuck run.
- [x] Prefer resolved unconsumed ticket, then open ticket, then retryable resolving/failed ticket.
- [x] Implement persisted next-cycle calculation from latest stuck run escalation cycle.
- [x] Ensure helpers do not mutate state except explicit recovery helpers if already part of store semantics.
- [x] Add fake-store or SQLite-backed unit tests.
- [x] Run formatter.
- [x] Run the acceptance command. Cargo rejected the combined two-filter command, so equivalent filters were run separately.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Latest stuck run ordering.
- [x] Task-scoped ticket selection.
- [x] Stale ticket rejection.
- [x] Resolved-unconsumed preference.
- [x] Open ticket fallback.
- [x] Retryable resolving/failed ticket handling.
- [x] Persisted next-cycle calculation.
- [x] Lease recovery compatibility.

## Acceptance Command

- [x] `cargo test state::supervisor_queries orchestrator::supervisor_state`

Notes:

- Cargo rejected the literal two-filter command with `unexpected argument 'orchestrator::supervisor_state'`.
- Equivalent commands passed: `cargo test state::supervisor_queries` and `cargo test orchestrator::supervisor_state`.

## Review Checklist

- [x] Reviewer verifies ordering is deterministic.
- [x] Reviewer verifies helpers preserve resumability after process interruption.
- [x] Reviewer verifies loop semantics remain in Workstream D.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review notes:

- Separate review subagent passed Workstream E with no findings.
- Local verification passed: `cargo fmt --check`, `cargo test state::supervisor_queries`, `cargo test orchestrator::supervisor_state`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes. The literal command is invalid Cargo syntax, so the intended filters were run separately.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
