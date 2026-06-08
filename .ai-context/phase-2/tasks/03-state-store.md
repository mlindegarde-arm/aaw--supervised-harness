# Workstream 3: State Store

## Scope

Implement SQLite migrations, repositories, leases, status transitions, ID persistence, and task/ticket/run/attempt/artifact/event storage.

## Owned Files

- [x] `src/state/**`
- [x] Embedded migration files, if stored outside `src/state`.

## Shared Files

- [ ] Domain structs only through coordinated contract updates.

## Interfaces Consumed

- [x] Domain structs, IDs, statuses, and errors from Workstream 0.
- [x] State directory paths from Workstream 2.

## Interfaces Produced

- [x] SQLite connection initialization with required pragmas.
- [x] Embedded transactional migrations.
- [x] `TaskStore` implementation.
- [x] Lease acquisition, heartbeat, expiration, and recovery APIs.
- [x] Repository methods needed by service/orchestrator.

## Tests To Add

- [x] Migration tests for fresh database creation.
- [x] Repository CRUD tests for tasks, validation commands, runs, attempts, tickets, resolutions, artifacts, and events.
- [x] Status transition tests.
- [x] Lease acquisition, conflict, expiration, reclaim, and heartbeat tests.
- [x] Idempotent ticket uniqueness tests.

## Acceptance Command

- [x] `cargo test state`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.
- [x] Workstream 2.

## Implementation Checklist

- [x] Create schema exactly matching `design.md`.
- [x] Add required unique constraints and indexes.
- [x] Enable `PRAGMA foreign_keys=ON`, `journal_mode=WAL`, and `busy_timeout=5000`.
- [x] Track migrations in `schema_migrations` with checksum.
- [x] Implement task status transitions and reject invalid transitions.
- [x] Implement run and ticket status transitions.
- [x] Implement task validation command ordering with uniqueness.
- [x] Implement lease owner, TTL, heartbeat, conflict exit semantics, and reclaim.
- [x] Implement startup recovery for expired running runs.
- [ ] Redact model/log/provider content before persistence through the security interface where applicable. Deferred to Workstream 6/8 integration; state now enforces lease ownership but still persists caller-provided redacted strings.

## Review Checklist

- [x] Confirm schema columns match `design.md`.
- [x] Confirm status transitions are enforced transactionally.
- [x] Confirm task/ticket/run mutations are lease-aware.
- [x] Confirm idempotent stuck ticket creation key is implemented.
- [x] Confirm tests run without real network or external services.

## Review Findings

- [x] Enforce active lease ownership on task-related mutations.
- [x] Require a resolved unconsumed ticket resolution before `stuck -> running`.
- [x] Include `blocked_on` in the idempotent stuck-ticket key.
- [x] Make ticket resolution insertion atomic with ticket `resolved` state.
- [x] Scope unconsumed resolution queries to resolved tickets.
- [x] Add regression tests for lease-aware mutation rejection, blocked-on ticket uniqueness, and resume preconditions.
- [x] Preserve active leases and task status in `update_task`.
- [x] Reject cross-task run, parent-run, attempt, artifact, event, and ticket references.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Store supports orchestrator and command surface needs.
- [x] Reviewer has passed this workstream.
