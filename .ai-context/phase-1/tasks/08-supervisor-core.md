# Workstream D: Supervisor Core

Design reference: `.ai-context/phase-2-design.md` sections `Supervisor Design`, `Loop Algorithm`, `Event and Output Contract`, and `Readiness Failures`.

## Scope

- [x] Implement the foreground supervisor loop.
- [x] Add create-and-supervise behavior.
- [x] Classify results into stable exit codes and progress events.

## Owned Files

- [x] `src/supervisor/**` or `src/orchestrator/supervisor.rs`
- [x] `src/service/supervisor.rs`
- [x] Supervisor unit tests

## Shared Files

- [x] Coordinate service trait changes through Workstream 0 if new methods are required.
- [x] Do not duplicate state selection logic from Workstream E.

## Blocked By

- [x] Workstream 0 option/event types passed review.
- [x] Workstream E state selection helpers passed review.

## Interfaces Consumed

- [x] `RunOrchestrator` run/resolve/resume behavior.
- [x] Workstream E supervisor-safe state helpers.
- [x] `SuperviseTaskOptions`
- [x] `SuperviseCreateOptions`
- [x] `SuperviseProgressEvent`
- [x] Cancellation token contract.

## Interfaces Produced

- [x] Service-level `supervise_task`.
- [x] Service-level `create_and_supervise_task`.
- [x] Typed event aggregation.
- [x] NDJSON-compatible progress events.
- [x] Actionable `next_commands`.

## Implementation Checklist

- [x] Inspect existing run, ticket resolve, and resume semantics.
- [x] Implement ready-task path: run task, complete or transition to ticket handling.
- [x] Implement stuck-task path: select ticket through Workstream E helpers.
- [x] Implement resolved-unconsumed path: resume without resolving again.
- [x] Implement open/retryable ticket path: resolve then resume.
- [x] Enforce `--max-cycles` capped by configured `max_escalation_cycles`.
- [x] Check cycle limit before any OpenAI-compatible provider call.
- [x] Preserve resolution consumption semantics: do not consume before resume provider send.
- [x] Implement active/expired running lease behavior.
- [x] Classify provider readiness, security, stuck limit, and internal failures into documented exit codes.
- [x] Generate progress events for run, stuck, resolve, resume, complete, and failure phases.
- [x] Generate actionable `next_commands` for nonzero results.
- [x] Implement create-and-supervise by creating a task then supervising it.
- [x] Add cancellation safe points.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Ready to complete.
- [x] Stuck to resolve to resume to complete.
- [x] Resolved unconsumed ticket resumes without resolving again.
- [x] Persisted cycle cap.
- [x] Resolving/failed ticket recovery.
- [x] Active and expired running lease behavior.
- [x] Provider readiness exit `20`.
- [x] Security exit `30`.
- [x] Cancellation safe points.
- [x] Resolution not consumed before resume provider send.

## Acceptance Command

- [x] `cargo test supervisor orchestrator::supervisor`

Notes:

- Cargo rejects the literal two-filter command with `unexpected argument 'orchestrator::supervisor'`.
- Equivalent commands passed: `cargo test supervisor` and `cargo test orchestrator::supervisor`.

## Review Checklist

- [x] Reviewer verifies loop follows persisted state, not process-local counters.
- [x] Reviewer verifies no OpenAI-compatible call occurs after cycle/security/provider preflight block.
- [x] Reviewer verifies event/final result contract matches design.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review findings to remediate:

- [x] Emit JSON progress as NDJSON on stderr in JSON mode through the runtime/output contract, not only inside final `data.progress_events`.
- [x] Fix final result aggregation so `resolved_tickets`/`resolution_ids` include only actual resolved tickets/resolutions, and include resolved-unconsumed selections.
- [x] Classify active resolving-ticket lease conflicts as active-lease failures with exit `1`, not stuck exit `10`.
- [x] Keep supervisor `result.events` limited to `SuperviseProgressEvent` payloads so JSON-mode stderr remains pure NDJSON and never falls back to prose rendering.

Remediation verification:

- [x] `cargo fmt`
- [x] `cargo fmt --check`
- [x] `cargo test supervisor`
- [x] `cargo test orchestrator::supervisor`
- [x] `cargo test runtime::tests::json_sink`
- [x] `cargo check`

2026-05-14 remediation notes:

- Supervisor aggregation no longer merges child command events into supervisor results and no longer appends terminal prose-only `supervise.complete` / `supervise.stuck` / `supervise.failed` events.
- Added real supervisor/runtime JSON-mode coverage for a stuck -> resolve -> resume -> complete flow; the test asserts every stderr line parses as JSON, all stderr events are `supervise.phase`, and no `info:` / `warn:` / `error:` prose leaks.
- Focused verification passed: `cargo fmt --check`, `cargo test supervisor`, and `cargo test runtime::tests::json_sink`.
- Independent re-review passed after remediation and verified JSON supervise stderr, aggregation, and active resolving-ticket lease classification.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
