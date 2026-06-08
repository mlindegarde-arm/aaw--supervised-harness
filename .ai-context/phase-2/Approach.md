# Parallel Implementation Approach

## Purpose

This document defines the implementation process for the Harness MVP described in `design.md`. It is a coordination contract for generating workstream task lists, assigning subagents, reviewing their output, and advancing the project through dependency layers without file ownership conflicts.

`design.md` is authoritative for product scope and technical requirements. `plan.md` remains background context for the longer-term direction only.

## Operating Model

Implementation proceeds in dependency layers, matching the "How To Generate Parallel Task Lists" and "Implementation Order" sections in `design.md`.

1. Create task-list files for each parallelizable workstream.
2. Run implementation subagents against those task lists.
3. Require each implementation subagent to iterate until its assigned workstream is functional against its acceptance command.
4. Run a separate review subagent after each implementation subagent reports completion.
5. If review finds issues, assign a follow-up implementation subagent to address the feedback.
6. Repeat review and remediation until a separate review subagent passes the workstream.
7. Advance dependent workstreams only after their blocked-by dependencies have passed review.

The primary implementation rule is that no two parallel subagents own the same source file unless the overlap is explicitly listed as a shared contract file. Shared contract changes should be made in Phase 0, or coordinated as a small blocking update before dependent work continues.

## Workstream Task-List Files

Task lists should be stored under `.ai-context/tasks/` and named with an ordered prefix:

```text
.ai-context/tasks/00-scaffold-shared-contracts.md
.ai-context/tasks/01-cli-runtime.md
.ai-context/tasks/02-config-filesystem.md
...
```

Each file must use markdown to-do list syntax so a subagent can track progress in place:

```markdown
- [ ] Task description
```

Each workstream task-list file must include:

- Scope.
- Owned files.
- Shared files, if any.
- Interfaces consumed.
- Interfaces produced.
- Tests to add.
- Acceptance command.
- Blocked-by dependencies.
- Implementation checklist.
- Review checklist.
- Done criteria.

Each checklist item should be concrete enough for an implementation subagent to execute without rediscovering the whole design document. Link back to relevant `design.md` sections instead of copying large blocks when possible.

## Dependency Layers

Generate and execute workstreams by these layers:

| Layer | Workstreams | Gate |
| --- | --- | --- |
| 0 | 0. Scaffold/shared contracts | Project compiles with stable contracts and ownership map. |
| 1 | 2. Config/filesystem, 3. State store foundation, 5. Provider fakes, 11. Fixture skeleton | Contracts from Layer 0 are stable enough for parallel work. |
| 2 | 1. CLI/runtime, 4. Workspace manager, 5. Provider clients, 6. Security/redaction, 7. Prompt/patch contracts | Layer 1 interfaces needed by each stream are available. |
| 3 | 8. Orchestrator, 9. Interactive shell, 10. Doctor/diagnostics | Core config/state/workspace/provider/security/prompt contracts are usable. |
| 4 | 11. E2E fixtures/tests, acceptance scripts, polish | End-to-end command surface and fake providers are implemented. |

The numbered implementation order in `design.md` remains the default sequence for integrating work. Parallel execution is allowed only within a layer after its blocked-by dependencies are satisfied.

## Workstream Ownership

Initial ownership follows the Parallel Workstream Matrix in `design.md`:

| Workstream | Owns |
| --- | --- |
| 0. Scaffold/shared contracts | `src/domain`, `src/error`, module skeleton |
| 1. CLI/runtime | `src/cli`, `src/runtime` |
| 2. Config/filesystem | `src/config`, path helpers |
| 3. State store | `src/state`, migrations |
| 4. Workspace manager | `src/workspace` |
| 5. Providers/fakes | `src/providers` |
| 6. Security/redaction | `src/security` |
| 7. Prompt/patch contracts | `src/prompts`, `src/patch` |
| 8. Orchestrator | `src/orchestrator`, `src/service` |
| 9. Interactive shell | `src/interactive` |
| 10. Doctor/diagnostics | `src/doctor` |
| 11. E2E fixtures/tests | `tests`, `fixtures` |

If a workstream needs to edit files outside its ownership boundary, the subagent must stop and record the required contract change in its task list instead of making the edit unilaterally.

## Subagent Instructions

Each implementation subagent should receive:

- The relevant task-list file.
- `design.md` as the authoritative specification.
- The blocked-by dependencies and expected interfaces.
- The owned file list.
- The acceptance command it must run before completion.

Each implementation subagent must use a goal-driven loop:

1. Read the assigned task list and relevant `design.md` sections.
2. Inspect existing code before editing.
3. Implement the smallest coherent slice that advances the checklist.
4. Run targeted tests or checks.
5. Update the task list checkboxes as work is completed.
6. Iterate until the workstream meets its done criteria.
7. Report changed files, tests run, unresolved risks, and any contract changes needed by other workstreams.

Implementation subagents should not mark a task complete unless the associated acceptance command passes or a documented external blocker prevents execution.

## Review Loop

After an implementation subagent completes a workstream, spawn a separate review subagent. The reviewer must not be the same subagent that implemented the work.

The review subagent should:

- Review the diff against `design.md` and the workstream task list.
- Verify owned file boundaries and call out unapproved overlap.
- Run or inspect the stated acceptance command when feasible.
- Prioritize correctness, security, behavioral regressions, missing tests, and integration risks.
- Return findings with file and line references where possible.
- Explicitly state pass or fail.

If review fails:

1. Record the findings in the workstream task-list file.
2. Spawn an implementation subagent to address only the review feedback and any directly required follow-up.
3. Run the acceptance command again.
4. Spawn a fresh review subagent.
5. Repeat until the review passes.

No dependent layer should start from a failed workstream unless the failure is explicitly accepted as a non-blocking risk and documented in the dependent task list.

## Merge and Integration Gates

A workstream is considered ready for integration when:

- Its implementation checklist is complete.
- Its review checklist is complete.
- A separate review subagent has passed it.
- Its acceptance command has passed, or the failure is documented as an external blocker.
- It has not edited files outside its ownership boundary without coordination.
- Any produced interfaces are documented in the task-list file.

A layer is considered complete when every required workstream in that layer has passed review. Only then should the next layer be started broadly in parallel.

## Acceptance Commands

Acceptance commands should be narrow early and broaden as dependencies land.

Examples:

```sh
cargo fmt --check
cargo test -p harness domain
cargo test state
cargo test providers
cargo test
cargo test --test e2e
```

Task-list generation should choose realistic commands for the current layer. If a command cannot exist yet, the task list should state the temporary targeted check and the later full command that must pass before final MVP acceptance.

## Final MVP Gate

The project is not complete until the final acceptance criteria in `design.md` pass:

- `cargo test` passes hermetically.
- `cargo test --test e2e` passes hermetically.
- Required commands support `--output json`.
- Runtime exits only through `main()`.
- Fake-provider success, stuck, ticket resolve, and resume flows pass.
- Patch, command, provider URL, and redaction safety tests pass.
- Manual real-provider smoke instructions are documented separately from CI.

