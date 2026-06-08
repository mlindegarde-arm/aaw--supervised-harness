# Workstream F1: Supervisor CLI E2E

Design reference: `.ai-context/phase-2-design.md` sections `CLI Binary E2E Tests`, `NDJSON Assertions`, and `Redaction Assertions`.

## Scope

- [x] Add actual-binary supervise acceptance tests.
- [x] Validate NDJSON/final JSON contracts.
- [x] Validate redaction across outputs, SQLite, artifacts, and fake-provider requests.

## Owned Files

- [x] `tests/e2e.rs`
- [x] `tests/support/**`
- [x] `docs/test-owned/real-provider-smoke.md`

## Shared Files

- [x] Coordinate test support API changes with F0/F2.
- [x] Do not weaken production security behavior for tests.

## Blocked By

- [x] Workstream A passed review.
- [x] Workstream D passed review.
- [x] Workstream E passed review.
- [x] Workstream F0 passed review.

## Interfaces Consumed

- [x] Binary e2e support.
- [x] Supervisor CLI command.
- [x] Fake provider scripting.

## Interfaces Produced

- [x] Supervisor CLI e2e coverage.
- [x] Manual real-provider smoke document.

## Implementation Checklist

- [x] Implement success-after-ticket e2e: local stuck, ticket resolution, local valid patch.
- [x] Implement `supervise --create` e2e.
- [x] Implement stuck-limit e2e and assert no extra OpenAI call after cap.
- [x] Implement missing provider readiness e2e with exit `20`.
- [x] Implement security-block e2e with exit `30`.
- [x] Assert final stdout is exactly one JSON object in json mode.
- [x] Assert stderr is valid chronological NDJSON events in json mode.
- [x] Assert process exit code equals final `exit_code`.
- [x] Assert nonzero final results include `next_commands`.
- [x] Assert completed task state, consumed resolution, artifacts/manifests, and request order.
- [x] Add sentinel redaction fixture and scan stdout, stderr, SQLite, artifacts, fake-provider requests, and provider errors.
- [x] Update `docs/test-owned/real-provider-smoke.md`.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Success supervise e2e.
- [x] Create-and-supervise e2e.
- [x] Stuck-limit exit `10`.
- [x] Readiness exit `20`.
- [x] Security exit `30`.
- [x] Interrupted/resolved-unconsumed resume workflow.
- [x] NDJSON schema/order assertions.
- [x] Direct SQLite redaction assertions.
- [x] Artifact and fake-provider request redaction assertions.

## Acceptance Command

- [x] `cargo test --test e2e supervise`

## Review Checklist

- [x] Reviewer verifies actual binary is driven in every acceptance test.
- [x] Reviewer verifies no real provider calls are possible.
- [x] Reviewer verifies redaction assertions cover all required surfaces.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.

## Implementation Notes

- Added seven actual-binary supervisor e2e tests using `BinaryHarness`: success-after-ticket, `supervise --create`, resolved-unconsumed ticket resume, stuck cap, missing provider readiness, security block, and provider-error redaction.
- Fake provider support remains guarded to localhost URLs; the security-block test toggles the existing untrusted-localhost policy off instead of weakening production URL policy.
- Verification run: `cargo fmt --check`, `cargo test --test e2e supervise`, and `cargo test --test e2e binary_harness`.
- Full verification passed: `cargo test` with 241 unit tests, 21 e2e tests, and 2 fixture tests.
- Independent review passed with no F1 acceptance blockers.
