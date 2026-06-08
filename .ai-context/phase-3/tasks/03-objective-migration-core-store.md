# Workstream B1: Objective Migration And Core Store

Design reference: `.ai-context/phase-3/Design.md`, Workstream B1.

## Scope

Add migration version 2 and core objective persistence without modifying migration version 1.

## Owned Files

- `src/state/mod.rs`
- `src/state/objective_queries.rs` if split
- State migration tests

## Shared Files

- `src/domain/**` only to consume Workstream 0 types.

## Blocked By

- Workstream 0.

## Interfaces Consumed

- Objective IDs and statuses.
- Existing SQLite migration framework.

## Interfaces Produced

- Migration version 2 named `objective_state`.
- Objective core CRUD.
- Objective list/get queries.

## Implementation Checklist

- [x] Add migration version 2 without changing migration version 1.
- [x] Add `objectives`.
- [x] Add `objective_plans`.
- [x] Add `objective_acceptance_criteria`.
- [x] Add `objective_validation_commands`.
- [x] Add required indexes for objective list/get performance.
- [x] Implement objective create/list/get/update status queries.
- [x] Preserve existing task/ticket schema compatibility.
- [x] Expose objective query methods through the existing store abstraction.

## Tests To Add

- [x] Fresh database applies migration 2.
- [x] Existing migration 1 database upgrades to migration 2.
- [x] Migration 1 checksum or SQL remains unchanged.
- [x] Objective CRUD tests.
- [x] Objective list status filtering tests.

## Acceptance Command

```sh
cargo test objective_migration
cargo test objective_store
```

## Review Checklist

- [x] Migration 1 is untouched.
- [x] New schema matches the Phase 3 design names and wire statuses.
- [x] Queries are transactional where required.
- [x] No service/runtime behavior is added in this workstream.

## Done Criteria

- [x] Acceptance command passes.
- [x] B2 can build transactions and scheduling queries on top of this store layer.
- [x] A separate review subagent passes this workstream.

## Review Notes

- 2026-05-14: Separate review subagent found no issues. `cargo test objective_migration` and `cargo test objective_store` passed in review.
