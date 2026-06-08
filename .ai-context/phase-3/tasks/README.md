# Phase 3 Task Lists

These task lists are generated from `.ai-context/phase-3/Design.md`.

## Execution Model

- Each workstream file is owned by one implementation subagent at a time.
- The implementation subagent should use the goal-driven approach: keep iterating until the workstream acceptance command passes or the blocker is documented in the task file.
- After implementation, a separate review subagent reviews the diff against the task file and design.
- Review findings are recorded in the same workstream file.
- If review fails, an implementation subagent fixes the findings and a fresh review subagent re-reviews.
- A workstream is complete only after its done criteria are checked and review passes.

## Dependency Layers

1. `00-shared-contracts-test-fakes.md`
2. `01-planner-resolver-schemas.md`, `03-objective-migration-core-store.md`, `05-objective-command-catalog-completion.md`
3. `02-validation-command-safety.md`, `04-objective-transactions-artifacts-scheduling.md`
4. `06-objective-runtime-json-output.md`
5. `07-objective-service-generated-tasks.md`
6. `08-objective-monitor-scheduler.md`
7. `09-objective-ticket-resolver.md`, `10-objective-validation-acceptance-repair.md`
8. `11-prompt-first-composer-slash-adapter.md`, `12-objective-dashboard-panels.md`
9. `13-binary-pty-e2e.md`

Parallel work is allowed within a layer once all listed dependencies are complete and reviewed.

