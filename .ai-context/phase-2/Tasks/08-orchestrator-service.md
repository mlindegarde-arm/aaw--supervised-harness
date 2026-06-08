# Workstream 8: Orchestrator and Service

## Scope

Implement the RWL loop, task creation/list/get/run, stuck ticket creation, ticket resolution, resume semantics, artifact recording, state transitions, and service methods consumed by CLI and interactive shell.

## Owned Files

- [x] `src/orchestrator/**`
- [x] `src/service/**`

## Shared Files

- [x] None unless service trait contracts require coordinated updates.

## Interfaces Consumed

- [x] Task store from Workstream 3.
- [x] Workspace manager and command runner from Workstream 4.
- [x] Providers and fakes from Workstream 5.
- [x] Security/redaction from Workstream 6.
- [x] Prompt and patch contracts from Workstream 7.
- [x] CLI/runtime output contracts from Workstream 1.

## Interfaces Produced

- [x] `HarnessService` implementation.
- [x] Task lifecycle use cases.
- [x] Ticket lifecycle use cases.
- [x] Resume use case.
- [x] Workspace prune and task cleanup use cases are supported at the workspace layer; no MVP CLI command is required.
- [x] Artifact and event creation orchestration.

## Tests To Add

- [x] Unit tests for successful fake Ollama patch flow.
- [x] Tests for validation-failure stuck ticket creation.
- [x] Tests for valid STUCK response ticket creation.
- [x] Tests for invalid response stuck-without-retry behavior.
- [x] Tests for patch apply failure retry and stuck behavior.
- [x] Tests for OpenAI advisory ticket resolution.
- [x] Tests for resume consuming a ticket resolution and creating a child run.
- [x] Tests for max escalation cycle behavior.

## Acceptance Command

- [x] `cargo test orchestrator service`
- [x] `cargo test`

## Blocked By

- [x] Workstream 3.
- [x] Workstream 4.
- [x] Workstream 5.
- [x] Workstream 6.
- [x] Workstream 7.

## Implementation Checklist

- [x] Implement `task create` behavior requiring at least one validation command.
- [x] Implement `task run` lease acquisition and worktree setup.
- [x] Implement bounded local Ollama attempts.
- [x] Apply valid patches and run validation commands.
- [x] Record prompts, responses, diffs, attempts, events, and artifact rows.
- [x] Mark task/run complete only when validation passes.
- [x] Create or return idempotent stuck tickets for all stuck conditions.
- [x] Compute failure fingerprints from normalized evidence.
- [x] Implement `ticket resolve` using OpenAI-compatible provider and advisory-only persistence.
- [x] Implement `resume` selection of latest resolved unconsumed ticket unless `--ticket` is supplied.
- [x] Mark selected ticket resolution consumed after inclusion in the next Ollama prompt.
- [x] Create child runs with `parent_run_id`.
- [x] Stop `harness run` after `max_escalation_cycles`.

## Review Checklist

- [x] Confirm state transitions match `design.md`.
- [x] Confirm all mutations are lease-aware.
- [x] Confirm OpenAI output is never directly applied.
- [x] Confirm stuck ticket idempotency is implemented.
- [x] Confirm artifacts are redacted before persistence.
- [x] Confirm retry counters match `design.md`.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Core RWL loop, ticket resolution, and resume are functional with fakes.
- [x] Reviewer has passed this workstream.
