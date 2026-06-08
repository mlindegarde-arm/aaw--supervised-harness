# Workstream F: Objective-Aware Ticket Resolver

Design reference: `.ai-context/phase-3/Design.md`, Workstream F.

## Scope

Implement objective-aware remote ticket resolution that packages objective context, validates guidance-only resolver output, persists exchange artifacts, and creates existing ticket resolutions without allowing remote patches.

## Owned Files

- `src/prompts/**`
- `src/planner/**`
- `src/orchestrator/objective.rs` integration through resolver trait only

## Shared Files

- `src/state/**` only through B2 resolver attempt and exchange APIs.
- Existing ticket resolution creation APIs.

## Blocked By

- Workstream A.
- Workstream B2.
- Workstream E trait boundary.

## Interfaces Consumed

- Strict resolver schema.
- Objective context packer.
- Resolver attempt lease APIs.
- Existing ticket and ticket resolution persistence.

## Interfaces Produced

- Objective-aware ticket resolver implementation.
- Resolver request context builder.
- Resolver exchange/artifact persistence.
- Idempotent resolver attempt handling.

## Implementation Checklist

- [x] Build resolver request including objective prompt, summary, acceptance criteria, status, ticket details, task context, latest artifacts, and prior resolutions.
- [x] Persist resolver request and response as objective artifacts.
- [x] Persist `planner_exchanges(kind='ticket_resolution')`.
- [x] Validate resolver response through strict resolver schema.
- [x] Reject patch-like, diff-like, or script-like resolver output.
- [x] Create existing ticket resolution guidance rows.
- [x] Mark resolver attempts resolved or failed.
- [x] Ensure duplicate resolver leases cannot resolve the same ticket concurrently.
- [x] Emit resolver lifecycle objective events.

## Tests To Add

- [x] Resolver prompt includes objective and ticket context.
- [x] Resolver output with patch-like text is rejected.
- [x] Resolver response creates ticket resolution guidance.
- [x] Resolver call count is bounded for an eligible ticket.
- [x] Resolver failure is persisted and does not loop infinitely.

## Acceptance Command

```sh
cargo test objective_ticket_resolver
cargo test resolver_attempt
cargo test ticket_resolution
```

## Review Checklist

- [x] Resolver output never enters patch parsing/application.
- [x] Existing ticket lifecycle remains source of truth.
- [x] Idempotence works across process restarts.
- [x] Resolver artifacts are redacted and context manifests are persisted.

## Done Criteria

- [x] Acceptance command passes.
- [x] Objective monitor can resume local work after resolver guidance is consumed.
- [x] A separate review subagent passes this workstream.

## Progress Log

- 2026-05-14: Implemented objective-aware ticket resolver integration in `src/orchestrator/objective.rs`. Resolver requests now include redacted objective, task, ticket, acceptance criteria, status, prior resolution, and current diff context. The strict resolver schema is enforced before guidance is written to existing `ticket_resolutions`; patch-like resolver content is rejected and persisted as a failed resolver attempt plus rejected `ticket_resolution` planner exchange. Added objective artifact persistence, resolver lifecycle events, resolver attempt leasing, and monitor resume of the local worker after guidance is created. Verified with `cargo test objective_ticket_resolver --lib`, `cargo test resolver_attempt --lib`, and `cargo test ticket_resolution --lib`.
- 2026-05-14: Review found failed resolver attempts could retry after restart through the objective orchestrator. Remediated by treating any failed objective resolver attempt as terminal for that ticket in the objective path and added restart regression coverage. Re-review passed with no findings.
