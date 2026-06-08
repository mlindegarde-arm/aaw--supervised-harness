# Workstream H: Prompt-First Composer And Slash Command Adapter

Design reference: `.ai-context/phase-3/Design.md`, Workstream H.

## Scope

Convert the TUI prompt into a Codex-like prompt-first composer while preserving slash-command access to harness commands, completion, compatibility warnings, and shell escapes.

## Owned Files

- `src/tui/composer.rs`
- `src/tui/input.rs`
- `src/tui/app.rs`
- Completion adapter tests

## Shared Files

- `src/runtime/**` only through command parsing/completion interfaces.
- `src/service/**` only through objective start/supervise stubs.

## Blocked By

- Workstream C1.
- Workstream C2.
- Workstream D service stubs.

## Interfaces Consumed

- Objective command runtime.
- Command catalog completion.
- Objective service start/supervise entrypoint.
- Existing shell escape behavior.

## Interfaces Produced

- `InputMode`.
- Raw prompt objective start path.
- Slash command adapter.
- Slash completion offset mapping.
- Compatibility warning for legacy command roots.

## Implementation Checklist

- [x] Add `InputMode` for plain prompt, slash command, shell escape, and disabled/running states.
- [x] Route plain prompt with no active objective to `objective start --supervise`.
- [x] Disable prompt editing while an objective is actively running, except cancellation controls.
- [x] Route `/...` through existing runtime command parser.
- [x] Implement slash-command completion offset mapping.
- [x] Ensure plain `ta<Tab>` does not command-complete.
- [x] Show compatibility warning for plain known command roots like `task list`.
- [x] Preserve `!git status --short` shell escape behavior.
- [x] Keep cursor and wrapping behavior from Phase 2 intact.
- [x] Add transcript entries for prompt routing decisions.

## Tests To Add

- [x] Composer unit tests for input mode detection.
- [x] Slash completion offset tests.
- [x] Plain prompt starts objective.
- [x] Plain known command root shows compatibility warning.
- [x] Shell escape regression test.
- [x] Active running objective disables prompt editing except cancellation.

## Acceptance Command

```sh
cargo test tui_composer
cargo test slash_completion
cargo test prompt_first
```

## Review Checklist

- [x] TUI no longer encourages command construction as the primary workflow.
- [x] Manual commands remain available behind `/`.
- [x] Existing completion infrastructure is reused.
- [x] Text wrapping and cursor visibility remain usable.

## Done Criteria

- [x] Acceptance command passes.
- [x] PTY tests can exercise prompt-first and slash-command flows.
- [x] A separate review subagent passes this workstream.
