# Workstream B2: Objective Transactions, Artifacts, And Scheduling Queries

Design reference: `.ai-context/phase-3/Design.md`, Workstream B2.

## Scope

Complete objective persistence by adding artifacts, messages, planner exchanges, resolver attempts, generated task relationships, atomic plan bundle transactions, and scheduling queries.

## Owned Files

- `src/state/**`
- State transaction and scheduling tests

## Shared Files

- `src/domain/**` only for consumed contracts.
- `src/planner/**` only for consumed schema structs.

## Blocked By

- Workstream B1.
- Workstream A schema structs.

## Interfaces Consumed

- Objective core store.
- Planner task/acceptance/validation schema structs.
- Existing task and ticket persistence.

## Interfaces Produced

- Objective artifact storage.
- Objective messages.
- Planner exchanges.
- Objective ticket resolver attempt storage and lease queries.
- Atomic `create_objective_plan_bundle`.
- Objective task dependencies.
- Ready-task scheduling queries.
- Monitor lease methods.

## Implementation Checklist

- [x] Add objective artifact persistence with redacted byte metadata.
- [x] Add objective messages with monotonic sequence per objective.
- [x] Add planner exchanges for `initial_plan` and `ticket_resolution`.
- [x] Add objective ticket resolver attempt rows and lease acquisition/release queries.
- [x] Add `objective_tasks`.
- [x] Add `objective_task_dependencies`.
- [x] Add `objective_task_validation_commands`.
- [x] Add `objective_events`.
- [x] Implement `reject_objective_plan` transaction.
- [x] Implement `create_objective_plan_bundle` as one transaction.
- [x] Ensure rejected plans create no tasks.
- [x] Implement ready-task query respecting dependency completion and sequence.
- [x] Implement objective monitor lease acquisition, refresh, and release.

## Tests To Add

- [x] Bundle transaction success inserts plan, criteria, validations, tasks, dependencies, events, and exchanges.
- [x] Bundle transaction failure rolls back all inserted rows.
- [x] Rejected plan creates no tasks.
- [x] Ready-task query respects dependencies.
- [x] Ready-task query preserves deterministic ordering.
- [x] Resolver attempt lease prevents duplicate resolver work.
- [x] Monitor lease conflict tests.

## Acceptance Command

```sh
cargo test objective_plan_bundle
cargo test objective_scheduling
cargo test objective_resolver_attempt
```

## Review Checklist

- [x] Bundle creation is atomic.
- [x] Resolver attempt idempotence is persisted, not memory-only.
- [x] Existing ticket and ticket resolution tables remain the source of truth for ticket lifecycle.
- [x] Scheduling queries do not imply parallel worker execution in Phase 3.

## Done Criteria

- [x] Acceptance command passes.
- [x] Service and monitor workstreams can depend on the store APIs without schema edits.
- [x] A separate review subagent passes this workstream.

## Progress Log

- 2026-05-14: Added B2 migration, objective artifact/message/exchange/event/task/dependency/resolver-attempt storage, atomic plan accept/reject transactions, ready-task scheduling query, monitor lease methods, and focused state tests. Acceptance filters pass locally; separate review subagent remains pending.
- 2026-05-14: First review found missing cross-objective invariants in accepted/rejected plan bundles, planner exchange artifact/ticket ownership checks, weak initial-plan exchange validation, and resolver lease release allowing expired leases. Remediated by enforcing objective ownership for messages, events, artifacts, dependencies, ticket-linked resolver attempts, accepted initial-plan exchanges, and unexpired resolver leases.
- 2026-05-14: Verified with `cargo test objective_plan_bundle`, `cargo test objective_scheduling`, `cargo test objective_resolver_attempt`, `cargo test objective_artifacts_events`, `cargo test objective_monitor_lease`, `cargo test objective_migration`, and `cargo test objective_store`.
- 2026-05-14: Second review found stale-plan scheduling, resolver release exchange ownership, and standalone message artifact ownership gaps. Remediated by scoping ready-task scheduling to `objectives.active_plan_id`, validating ticket-resolution exchange objective/ticket/kind before resolver release, and reusing message artifact ownership checks in standalone insertion. Verified with `cargo test objective_scheduling`, `cargo test objective_resolver`, `cargo test objective_message_insert`, `cargo test objective_artifacts_events`, and `cargo test objective_plan_bundle`.
- 2026-05-14: Re-review passed with no findings. Reviewer also ran `cargo test objective_monitor_lease`.
