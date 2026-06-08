# Workstream 0: Interface Seed and Contracts

Design reference: `.ai-context/phase-2-design.md` sections `Parallel Workstreams`, `Command Surface Changes`, `TUI Design`, and `Supervisor Design`.

## Scope

- [x] Seed the shared Phase 2 contracts needed by later parallel workstreams.
- [x] Keep implementations as placeholders where behavior belongs to later workstreams.
- [x] Preserve existing command behavior while new interfaces compile.

## Owned Files

- [x] `src/runtime/catalog.rs`
- [x] `src/runtime/supervise.rs`
- [x] `src/runtime/events.rs`
- [x] `src/service/supervisor.rs`
- [x] Module export lines in `src/runtime/mod.rs`
- [x] Module export lines in `src/service/mod.rs`

## Shared Files

- [x] Coordinate any additional module export changes before editing.
- [x] Do not migrate existing command parsing beyond the minimum needed for type references.

Notes:

- Added default `HarnessService` supervisor methods in `src/service/mod.rs` so existing service implementations continue compiling while Workstream D owns real loop behavior.
- The requested acceptance command `cargo test runtime::supervise service::supervisor` is not valid Cargo syntax because Cargo accepts a single test filter. Equivalent filters were run separately.

## Blocked By

- [x] No dependencies.

## Interfaces Produced

- [x] `SuperviseTaskOptions`
- [x] `SuperviseCreateOptions`
- [x] `supervise_task` service trait signature
- [x] `create_and_supervise_task` service trait signature
- [x] Structured command metadata types
- [x] `SuperviseProgressEvent`
- [x] TUI-facing `TranscriptEvent`
- [x] TUI-facing `PaneStateSnapshot`
- [x] Cancellation token contract
- [x] Supervisor state-view/store trait signatures

## Implementation Checklist

- [x] Inspect existing `runtime`, `service`, `domain`, `orchestrator`, and `state` modules before editing.
- [x] Add runtime event types without changing existing `CommandEvent` output semantics.
- [x] Add supervise option structs using existing domain ID types.
- [x] Add command schema metadata types without requiring full command migration.
- [x] Add service extension points with placeholder implementations returning stable `CommandResult` failures where behavior is not implemented yet.
- [x] Add supervisor state/query trait signatures with no SQLite behavior yet.
- [x] Add compile-focused tests for new types and placeholder behavior.
- [x] Run formatter.
- [ ] Run the acceptance command. Blocked: exact command is rejected by Cargo as invalid two-filter syntax; ran `cargo test runtime::supervise` and `cargo test service::supervisor` instead.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Runtime type construction tests for supervise options/events.
- [x] Service placeholder tests that prove the new trait surface compiles and returns deterministic failure until Workstream D.
- [x] Command metadata construction smoke test.

## Acceptance Command

- [ ] `cargo test runtime::supervise service::supervisor`

## Review Checklist

- [x] Reviewer verifies no existing command behavior changed.
- [x] Reviewer verifies shared contract names and fields match `phase-2-design.md`.
- [x] Reviewer verifies later workstreams can consume interfaces without owning the same files.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review notes:

- Separate review subagent passed Workstream 0 with no blocking findings.
- Local verification passed: `cargo test runtime::supervise`, `cargo test service::supervisor`, `cargo test runtime::events`, `cargo test runtime::catalog`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes. The literal command is invalid Cargo syntax, so the intended filters were run separately.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
