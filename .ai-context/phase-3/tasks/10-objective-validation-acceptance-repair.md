# Workstream G2: Objective Validation And Acceptance Repair

Design reference: `.ai-context/phase-3/Design.md`, Workstream G2.

## Scope

Run objective-level validation commands, update acceptance status, create local acceptance repair tasks for failing validation, and decide final objective completion/blocking.

## Owned Files

- `src/orchestrator/objective.rs`
- `src/service/**`
- State queries for validation status

## Shared Files

- `src/security/**` only through validation policy APIs.
- `src/state/**` only through objective validation APIs.

## Blocked By

- Workstream E.
- Workstream G1.

## Interfaces Consumed

- Objective monitor lifecycle.
- Validation command safety policy.
- Objective validation command rows.
- Existing task creation and local supervision behavior.

## Interfaces Produced

- Objective validation execution.
- Acceptance repair task creation.
- Objective completion/blocking rules.

## Implementation Checklist

- [x] Select trusted executable objective validation commands.
- [x] Refuse or block on required `needs_review` or `rejected` validation commands.
- [x] Run validation commands through existing command runner and cancellation handling.
- [x] Persist validation artifacts and acceptance status updates.
- [x] Mark objective complete when all acceptance criteria pass.
- [x] Create or reuse acceptance repair task on validation failure.
- [x] Ensure repair task consumes validation artifacts as context.
- [x] Continue monitor loop after repair success.
- [x] Block deterministically when repair budget is exhausted or repair gets stuck.
- [x] Emit validation and repair objective events.

## Tests To Add

- [x] Passing validation completes objective.
- [x] Failed validation creates one repair task.
- [x] Repair success completes objective.
- [x] Repair stuck path creates ticket or blocks deterministically.
- [x] Unsafe validation command blocks execution.
- [ ] Cancellation during validation is deterministic.

## Acceptance Command

```sh
cargo test objective_validation
cargo test acceptance_repair
```

## Review Checklist

- [x] Unsafe planner validation commands are never executed.
- [x] Repair task creation is idempotent.
- [x] Objective terminal states match the failure matrix.
- [x] Validation artifacts are available for TUI/CLI inspection.

## Done Criteria

- [x] Acceptance command passes.
- [x] End-to-end objective success and repair scenarios can be tested.
- [x] A separate review subagent passes this workstream.

## Progress Log

- 2026-05-14: Implemented objective-level validation and acceptance repair in the monitor. When all active objective tasks complete, the monitor now runs trusted objective validation commands through the existing command runner, rechecks command safety before execution, persists failure logs as objective artifacts, updates acceptance criteria to passing/failing, marks the objective complete on acceptance pass, or creates one idempotent acceptance repair task that includes the failing command and artifact path in its goal. Repair success loops back into validation and completes the objective. Added focused tests for passing validation, repair task idempotence, repair success, and unsafe validation blocking. Verified with `cargo test objective_validation --lib` and `cargo test acceptance_repair --lib`.
- 2026-05-14: Review found validation failure artifact overwrite, missing runner-error events, and runtime repo/cwd handling gaps. Remediated with unique validation failure artifact names, persisted validation-failed events for runner errors, runtime repo propagation into validation execution and repair task creation, and regression tests. Final re-review passed with no findings. Residual gap: cancellation during an in-flight validation command remains future work because the current `CommandRunner` API is synchronous.
