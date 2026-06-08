# Workstream D: Objective Service And Generated Task Creation

Design reference: `.ai-context/phase-3/Design.md`, Workstream D.

## Scope

Implement objective start and detail service behavior: read prompt input, call the remote planner once, validate the plan, classify validation commands, and create generated tasks through the atomic bundle transaction.

## Owned Files

- `src/service/**`
- Generated task creation adapters

## Shared Files

- `src/planner/**` only through schema/prompt interfaces.
- `src/state/**` only through B2 store APIs.
- `src/security/**` only through G1 policy APIs.

## Blocked By

- Workstream A.
- Workstream B2.
- Workstream C2.
- Workstream G1.

## Interfaces Consumed

- Remote planner provider.
- Strict planner parser.
- Validation command policy.
- Atomic objective bundle transaction.
- Runtime service trait.

## Interfaces Produced

- `start_objective`.
- Prompt input handling.
- Objective detail/list/plan view models.
- Generated task creation adapter.

## Implementation Checklist

- [x] Implement prompt source resolution for variadic, `--prompt`, `--prompt-file`, and `--stdin`.
- [x] Create objective shell before planner call.
- [x] Persist planner request/response objective artifacts.
- [x] Validate planner response strictly.
- [x] Classify planner validation commands.
- [x] Map planner tasks to generated local task rows.
- [x] Create acceptance criteria, validations, tasks, dependencies, and events through bundle transaction.
- [x] Reject invalid plans without creating tasks.
- [x] Emit objective planning lifecycle events.
- [x] Implement objective list/get/plan view models.
- [x] Ensure provider call ledger shows exactly one planner call per objective version.

## Tests To Add

- [x] Fake planner success creates objective, plan, criteria, validations, tasks, dependencies, and events.
- [x] Planner rejection creates no tasks and emits `objective.plan_rejected`.
- [x] Unsafe validation output is blocked or marked non-executable as designed.
- [x] Prompt input source conflict tests.
- [x] Provider call ledger test for one planner call.

## Acceptance Command

```sh
cargo test start_objective
cargo test objective_service
cargo test generated_task
```

## Review Checklist

- [ ] No remote planner output is treated as executable code.
- [ ] Objective creation is durable across process restart.
- [ ] Plan rejection leaves useful artifacts and diagnostics.
- [ ] Generated tasks are bounded and suitable for the local worker loop.

## Done Criteria

- [x] Acceptance command passes.
- [ ] Objective monitor can supervise generated tasks from the service.
- [x] A separate review subagent passes this workstream.

## Progress Log

- 2026-05-14: Implemented `DefaultHarnessService::start_objective` through the orchestrator. The flow resolves text/file prompts plus CLI `--stdin`, creates an objective shell before the planner call, builds a planner prompt, records exactly one remote planner request, validates the response with the repo-aware planner parser, classifies objective and task validation commands, writes redacted planner request/response objective artifacts, commits accepted plans through `create_objective_plan_bundle`, rejects unsafe/invalid plans without creating tasks, and emits objective planning lifecycle events. Added fake-provider orchestrator tests for successful generated task creation, plan view output, and unsafe validation rejection. Verified with `cargo test start_objective`, `cargo test generated_task`, and `cargo test objective_service`.
- 2026-05-14: First review found three issues: planner provider failures left objective shells in `planning`, rejected planner output persisted state but returned no live lifecycle events, and `--max-worker-attempts` was ignored for generated tasks. Remediated by persisting failed planner exchanges/events and marking objectives failed on provider errors, returning failure `CommandResult`s with `objective.plan_rejected`/`objective.failed` events, and applying `max_worker_attempts` to generated task budgets. Added regression assertions and verified with `cargo test start_objective`.
- 2026-05-14: Re-review passed with no findings. Reviewer ran `cargo test start_objective`, `cargo test objective_service`, and `cargo test generated_task`.
