# Workstream E: Objective Monitor Scheduler

Design reference: `.ai-context/phase-3/Design.md`, Workstream E.

## Scope

Implement the foreground objective monitor loop that leases objectives, schedules generated tasks sequentially, delegates to existing local task supervision, detects eligible tickets, and enforces cancellation checkpoints.

## Owned Files

- `src/orchestrator/objective.rs`
- `src/service/**` monitor entrypoints

## Shared Files

- `src/state/**` only through B2 scheduling APIs.
- Existing task supervisor interfaces.

## Blocked By

- Workstream B2.
- Workstream D.
- Workstream 0.

## Interfaces Consumed

- Objective store scheduling queries.
- Existing `supervise_task` behavior.
- Objective events.
- Resolver trait boundary.
- Cancellation token/command runner support.

## Interfaces Produced

- Objective monitor lease behavior.
- `run_one_cycle`.
- Foreground supervise loop.
- Ticket eligibility detection through resolver trait.
- Cancellation checkpoints.

## Implementation Checklist

- [x] Add objective monitor entrypoint.
- [x] Acquire and refresh objective monitor lease.
- [x] Implement deterministic `run_one_cycle`.
- [x] Schedule ready generated tasks sequentially.
- [x] Delegate each task to existing local `supervise_task`.
- [x] Persist task attempt counts against objective tasks.
- [x] Detect eligible open tickets for objective tasks.
- [x] Call resolver trait only for eligible tickets.
- [x] Apply max worker attempts and max cycles.
- [x] Implement cancellation checkpoints from the design.
- [x] Emit objective lifecycle events.
- [x] Release leases on terminal state or cancellation.

## Tests To Add

- [x] Ready task scheduling respects dependencies.
- [x] Lease conflict prevents duplicate monitor work.
- [x] Local task completion advances objective state.
- [x] Stuck task creates or detects ticket and invokes resolver trait.
- [x] Cancellation during task/resolver/validation checkpoints is deterministic.
- [x] Max cycles and max worker attempts block predictably.

## Acceptance Command

```sh
cargo test objective_monitor
cargo test run_one_cycle
cargo test objective_cancellation
```

## Review Checklist

- [x] Phase 3 remains sequential even though parallel groups are stored.
- [x] Remote model is not called during normal local worker attempts.
- [x] Resolver calls are bounded and idempotent.
- [x] Monitor state is restartable from persisted data.

## Done Criteria

- [x] Acceptance command passes.
- [x] Objective-aware resolver and validation workstreams can integrate through traits.
- [x] A separate review subagent passes this workstream.

## Progress Log

- 2026-05-14: Added `src/orchestrator/objective.rs` with foreground `supervise_objective`, objective monitor leases, deterministic `run_one_objective_cycle`, sequential ready-task scheduling, delegation to existing task `supervise_task`, terminal complete/blocked/failed/cancelled objective state updates, and objective lifecycle events. Added active-plan task status counts to the objective store so the monitor can distinguish all-complete from blocked/no-ready. Added tests for generated task completion, one-cycle scheduling, monitor lease behavior, and cancellation. Verified with `cargo test objective_monitor`, `cargo test run_one_cycle`, and `cargo test objective_cancellation`. Remaining E work is objective-aware resolver integration and persisted objective task attempt counters.
- 2026-05-14: Finished objective monitor resolver integration. The monitor now records objective task worker attempts from supervised run artifacts, detects stuck objective tasks and retryable tickets, invokes the objective-aware resolver only when there is no resolved unconsumed guidance, then resumes the local worker with the resolved ticket. Added coverage for stuck ticket resolution and local-worker resume. Verified with `cargo test objective_monitor --lib`, `cargo test run_one_cycle --lib`, and `cargo test objective_cancellation --lib`.
- 2026-05-14: Review found restart/bounding gaps. Remediated failed resolver retry across restarts, persisted worker attempt budget enforcement before dispatch/resume, and expired running generated-task lease recovery before scheduling. Re-review passed with no findings.
