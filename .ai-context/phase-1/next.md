# Next Phase: Prompt-First Objective Planning

## Product Direction

The next phase should move `harness` from a command-first TUI to a prompt-first agent experience, closer to the Codex CLI.

Current Phase 2 behavior is command-first:

```text
> task create --title ...
> supervise task_...
```

The intended workflow is prompt-first:

```text
> Create a Rust clone of the Volt CLI in this repository: https://github.com/Arm-Volt/volt-cli
```

The user should not manually create tasks for normal use. Instead, `harness` should take the user prompt, add a system-level planning prompt, send it to the OpenAI-compatible/Codex-like remote model, and use the structured response to create objectives, acceptance criteria, validation commands, and local Ollama work items.

## Target Flow

```text
user objective prompt
  -> harness adds system planning instructions
  -> remote OpenAI-compatible planner returns structured plan
  -> harness validates and persists objective plan
  -> harness creates small local-worker tasks
  -> local Ollama Rust model executes tasks
  -> stuck workers create tickets
  -> monitor sees tickets and asks remote model for resolutions
  -> local workers resume with ticket guidance
  -> repeat until acceptance criteria pass
```

The remote model should define what "done" means from the original prompt. The local Ollama model should execute bounded Rust implementation tasks against those criteria.

## Example User Prompt

```text
Create a Rust clone of the Volt CLI in this repository: https://github.com/Arm-Volt/volt-cli
```

For this prompt, the remote planner should inspect or reason about the source project and return a structured plan that can drive local work. The response should include enough detail for `harness` to create tasks without the user manually writing `task create` commands.

## Planner Responsibilities

The OpenAI-compatible planner should produce structured output, not general prose.

It should define:

- Objective title and summary.
- Acceptance criteria.
- Validation commands or validation scripts.
- Task breakdown small enough for local Ollama workers.
- Task dependencies and suggested parallelism.
- Files or areas each task should own when known.
- Risks, unknowns, and research tasks.
- Final verification criteria.

The planner should not directly mutate the repository. `harness` remains responsible for validating the planner output, persisting state, creating tasks, supervising workers, and enforcing safety rules.

## Structured Plan Shape

The exact schema can evolve, but the plan should be machine-readable and validated before execution. A representative shape:

```json
{
  "objective": {
    "title": "Create Rust clone of Volt CLI",
    "summary": "Implement a Rust CLI with the core behavior and command surface of Arm-Volt/volt-cli.",
    "acceptance_criteria": [
      "Rust CLI builds successfully",
      "Core Volt CLI command groups are represented",
      "Help output exists for implemented commands",
      "Fixture tests validate representative workflows",
      "Final validation script passes"
    ],
    "validation_commands": [
      "cargo fmt --check",
      "cargo test",
      ".harness/validation/acceptance.sh"
    ]
  },
  "tasks": [
    {
      "title": "Inspect Volt CLI command surface",
      "goal": "Determine command groups, options, and key workflows from the source Volt CLI project.",
      "validation": ".harness/validation/command-surface-known.sh",
      "depends_on": []
    },
    {
      "title": "Implement Rust CLI skeleton",
      "goal": "Create a clap-based Rust CLI with command groups matching the planned Volt CLI surface.",
      "validation": "cargo test cli_surface",
      "depends_on": ["Inspect Volt CLI command surface"]
    }
  ]
}
```

## TUI Direction

The TUI should become an objective monitor and prompt surface, not primarily a command autocomplete shell.

Normal text should be interpreted as a user prompt for the remote planner:

```text
> Create a Rust clone of the Volt CLI in this repository: https://github.com/Arm-Volt/volt-cli
```

Harness commands should remain available, but should move behind an explicit command prefix so they are secondary:

```text
> /task list
> /ticket list
> /supervise task_...
```

Shell escapes should remain:

```text
> !git status --short
```

The command autocomplete built in Phase 2 should still be useful for slash commands, but it should not be the main user workflow.

## TUI Visualization Requirements

While the TUI is open, the user should be able to see the system working.

The UI should show:

- Current objective and acceptance criteria.
- Planner status and latest plan revision.
- Task queue grouped by ready/running/stuck/complete.
- Active local Ollama workers.
- Open/resolving/resolved tickets.
- Current remote planner/ticket-resolution calls.
- Supervisor cycles and next resume commands.
- Validation progress and final acceptance status.
- Transcript of meaningful events.

Representative display:

```text
Objective
  Create Rust clone of Volt CLI

Plan
  8 tasks total | 2 running | 1 stuck | 1 ticket resolving | 3 complete

Workers
  task_123 running  Implement CLI skeleton
  task_456 stuck    Auth command parity

Tickets
  ticket_789 resolving via OpenAI-compatible model

Transcript
  planner generated 8 tasks
  worker task_123 applied patch
  validation failed for task_456
  ticket_789 created
  ticket_789 resolved
  worker task_456 resumed
```

## New Concepts To Add

- `objective`: persisted top-level user request, plan, acceptance criteria, generated tasks, and status.
- `planner`: OpenAI-compatible provider path that turns a user prompt into a structured objective plan.
- `planner schema`: strict machine-readable response format with validation and actionable errors.
- `objective start`: command/API used by the TUI and CLI to start prompt-first planning.
- `objective monitor`: orchestration process that supervises generated local tasks and ticket resolution until done.
- `slash commands`: explicit command mode in the TUI for manual inspection and control.

## CLI Direction

Possible commands:

```sh
harness objective start "Create a Rust clone of the Volt CLI in this repository: https://github.com/Arm-Volt/volt-cli"
harness objective get <objective-id>
harness objective list
harness objective supervise <objective-id>
```

The no-command TUI should be the preferred interactive entry point:

```sh
harness --repo "$REPO"
```

Inside the TUI, a plain prompt should start objective planning. Slash commands should handle manual commands.

## Acceptance Criteria For The Next Phase

- A user can enter a plain-language objective in the TUI.
- `harness` sends the objective plus system planning instructions to the OpenAI-compatible planner.
- The planner response is strict structured data and is validated.
- `harness` persists the objective, acceptance criteria, validation commands, and generated tasks.
- Generated tasks can be supervised by the existing local Ollama worker loop.
- Tickets created by local workers are resolved through the OpenAI-compatible provider.
- Resolved tickets resume local workers automatically.
- The loop repeats until generated acceptance criteria pass or a clear blocked state is reached.
- The TUI shows visible progress across planning, task execution, tickets, resumes, and validation.
- Existing command-mode workflows remain available through slash commands or direct CLI commands.

## Safety And Control

- Remote planner output should be treated as untrusted structured intent.
- `harness` should validate schemas before creating state or scripts.
- Generated validation scripts should be reviewed or sandboxed before execution.
- OpenAI-compatible output remains advisory and should not directly apply patches.
- Local Ollama workers remain responsible for repo mutations through the existing patch safety path.
- Ticket resolutions should continue to be consumed only after being included in a local worker prompt.
