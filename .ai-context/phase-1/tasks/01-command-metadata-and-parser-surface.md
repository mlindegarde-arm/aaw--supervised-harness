# Workstream A: Command Metadata and Parser Surface

Design reference: `.ai-context/phase-2-design.md` sections `Command Surface Changes`, `Command Catalog Update`, and `Implementation Order`.

## Scope

- [x] Make structured command metadata authoritative for help and completion.
- [x] Add parser support for `supervise` and `supervise --create`.
- [x] Add schema/clap/parser parity tests.

## Owned Files

- [x] `src/runtime/catalog.rs`
- [x] Equivalent parser code in `src/runtime/mod.rs`
- [x] Minimal module wiring in `src/runtime/mod.rs`
- [x] Runtime parser tests

## Shared Files

- [x] Coordinate with Workstream 0 before changing metadata type shapes.
- [x] Avoid changing supervisor behavior; this stream only parses and exposes command metadata.

## Blocked By

- [x] Workstream 0 passed review.

## Interfaces Consumed

- [x] Command schema types from Workstream 0.
- [x] Supervise option structs from Workstream 0.

## Interfaces Produced

- [x] Metadata entries for all existing commands plus `supervise`.
- [x] Parsed command variant for `supervise <task-id>`.
- [x] Parsed command variant for `supervise --create`.
- [x] Parity assertions across schema, clap, and parser.

## Implementation Checklist

- [x] Inspect current runtime parser and command catalog behavior.
- [x] Add structured metadata entries for current commands without regressing help output.
- [x] Add metadata for meta commands `exit`, `quit`, and `help`.
- [x] Add `supervise <task-id>` parse support.
- [x] Add `supervise --create --title --goal --validation ...` parse support.
- [x] Enforce required create fields and option conflicts.
- [x] Preserve global options such as `--repo`, `--output`, and `--quiet`.
- [x] Add parity tests for command paths, globals, options, value kinds, repeatability, and required arguments.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Parser accepts documented `supervise` forms.
- [x] Parser rejects missing task id without `--create`.
- [x] Parser rejects `--create` missing title, goal, or validation.
- [x] Help/catalog exposes TUI/meta/supervise command surface.
- [x] Schema/clap/parser parity test passes.

## Acceptance Command

- [x] `cargo test runtime`

## Review Checklist

- [x] Reviewer verifies schema is the source used by help/completion-ready APIs.
- [x] Reviewer verifies parser behavior matches `phase-2-design.md`.
- [x] Reviewer verifies no supervisor loop behavior is implemented here.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review notes:

- Initial review found clap/schema accepted invalid `supervise --create` forms that the parser rejected.
- Remediation added conditional schema fields for clap requirements/conflicts and regression coverage.
- Fresh review subagent passed with no blocking findings.
- Local verification passed: `cargo fmt --check`, `cargo test runtime`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
