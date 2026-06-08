# Workstream G: Optional Shell Completion Export

Design reference: `.ai-context/phase-2-design.md` sections `Optional Shell Completion Export`, `Command Catalog Update`, and `Implementation Order`.

## Scope

- [x] Add `harness completions <bash|zsh|fish>` after schema/clap parity is green.
- [x] Keep this lower priority than interactive TUI completion and supervisor acceptance.

## Owned Files

- [x] `src/completion/shell.rs`
- [x] Minimal command wiring in `src/runtime/parser.rs` (implemented in `src/runtime/mod.rs`; no parser split exists)
- [x] Tests for generated completion command

## Shared Files

- [x] Coordinate parser wiring with Workstream A.
- [x] Coordinate completion module exports with Workstream B.

## Blocked By

- [x] Workstream A command metadata/clap alignment passed review.

## Interfaces Consumed

- [x] Command schema.
- [x] Clap command builder.

## Interfaces Produced

- [x] Installable shell completion output.
- [x] Shell completion tests.

## Implementation Checklist

- [x] Confirm shell completion export remains in Phase 2 scope before starting.
- [x] Add `harness completions <shell>` parse support.
- [x] Generate bash, zsh, and fish completions using `clap_complete` or equivalent.
- [x] Ensure generated completions reflect command schema.
- [x] Add tests for supported shells and unsupported shell errors.
- [x] Run formatter.
- [x] Run the acceptance command. (`cargo test completion::shell runtime` rejected by Cargo; split into accepted filters.)
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Bash completion output smoke.
- [x] Zsh completion output smoke.
- [x] Fish completion output smoke.
- [x] Unsupported shell returns usage error.
- [x] Schema/clap parity remains green.

## Acceptance Command

- [x] `cargo test completion::shell runtime` (split into `cargo test completion::shell` and `cargo test runtime`)

## Review Checklist

- [x] Reviewer verifies generated completions reflect the schema/clap surface.
- [x] Reviewer verifies this work did not destabilize interactive completion.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.

## Review Notes

- [x] Initial review failed because static schema values were only partially mapped into clap value parsers and parser validation missed task/ticket status filters.
- [x] Remediation applies all `ValueSource::Static` values generically to clap, validates task/ticket status filters in the hand parser, and extends shell completion smoke tests for static values.
- [x] Re-review passed after remediation.
- [x] Final verification: `cargo fmt --check`, `cargo test completion::shell`, `cargo test runtime`, `cargo test completion`, and full `cargo test` all passed. Full suite counts: 246 unit tests, 21 e2e tests, 2 fixture tests, 11 PTY tests, 0 doc tests.
