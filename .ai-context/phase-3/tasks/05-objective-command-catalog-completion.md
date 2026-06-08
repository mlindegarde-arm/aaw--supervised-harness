# Workstream C1: Objective Command Catalog And Completion

Design reference: `.ai-context/phase-3/Design.md`, Workstream C1.

## Scope

Add objective commands to the runtime catalog, parser, clap surface, and completion system while preserving Phase 2 command parity.

## Owned Files

- `src/runtime/catalog.rs`
- `src/runtime/mod.rs`
- `src/completion/**`
- `src/completion/shell.rs`

## Shared Files

- `src/domain/**` only to consume objective ID/status contracts.

## Blocked By

- Workstream 0.

## Interfaces Consumed

- Objective ID and status types.
- Existing command catalog and completion abstractions.

## Interfaces Produced

- Objective command group in catalog/parser/clap.
- `ObjectiveId` completion source.
- `ObjectiveStatus` completion source.
- Shell completion coverage for objective commands.

## Implementation Checklist

- [x] Add `objective start` command surface.
- [x] Add `objective list --status`.
- [x] Add `objective get`.
- [x] Add `objective plan`.
- [x] Add `objective validate --dry-run`.
- [x] Add `objective supervise`.
- [x] Add `objective cancel`.
- [x] Support variadic trailing prompt for `objective start`.
- [x] Enforce conflicts between variadic prompt, `--prompt`, `--prompt-file`, and `--stdin`.
- [x] Add objective status completion values.
- [x] Add dynamic objective ID completion hook.
- [x] Update shell completion generation tests.

## Tests To Add

- [x] Parser accepts all objective command shapes.
- [x] Parser rejects invalid status values.
- [x] Parser rejects conflicting prompt inputs.
- [x] Catalog/clap/completion parity tests.
- [x] bash/zsh/fish completion tests include objective commands and statuses.
- [x] Phase 2 command parity regression tests.

## Acceptance Command

```sh
cargo test objective_command
cargo test objective_parser
cargo test completion
cargo test catalog
```

## Review Checklist

- [x] Existing command roots keep their Phase 2 behavior.
- [x] Command catalog remains the single source for completion metadata.
- [x] Dynamic completion hooks do not require provider/model network calls.
- [x] Parser behavior matches the design examples.

## Done Criteria

- [x] Acceptance command passes.
- [x] Runtime dispatch workstream can route parsed objective commands.
- [x] A separate review subagent passes this workstream.

## Review Findings

- 2026-05-14: Initial review failed because production objective ID completion was not wired through the real TUI completion state, objective parser/clap parity missed `--option=value` forms, and the acceptance filters did not include the negative objective parser test.

## Remediation Notes

- 2026-05-14: Added production objective completion through the real TUI completion state, added assigned long-option parsing for objective options, and expanded acceptance filters to include `cargo test objective_parser`.

## Review Notes

- 2026-05-14: Fresh review passed after remediation. Reviewer verified production objective completion, assigned long-option parsing, acceptance filter coverage, and placeholder-only dispatch boundary.
