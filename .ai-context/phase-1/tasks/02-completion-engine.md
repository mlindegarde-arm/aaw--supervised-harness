# Workstream B: Completion Engine

Design reference: `.ai-context/phase-2-design.md` sections `Autocomplete Design`, `Completion Matrix`, and `Dynamic Completion Safety`.

## Scope

- [x] Build a UI-agnostic completion engine.
- [x] Support tolerant token/cursor parsing and command readiness.
- [x] Support dynamic task/ticket completions only through `CompletionStateView`.

## Owned Files

- [x] `src/completion/**`
- [x] Completion unit tests

## Shared Files

- [x] Consume command metadata from Workstream A rather than duplicating command lists.
- [x] Coordinate any required metadata changes through Workstream A or a small interface patch.

Notes:

- Added the small shared-contract module export `pub mod completion;` in `src/lib.rs` so Workstream B's produced interfaces compile and can be consumed. No runtime/catalog metadata changes were made.

## Blocked By

- [x] Workstream 0 interfaces available.
- [x] Workstream A metadata shape stable enough to consume.

## Interfaces Consumed

- [x] Command metadata/catalog.
- [x] Domain task/ticket IDs and statuses.
- [x] Redaction helpers.

## Interfaces Produced

- [x] `CompletionEngine`
- [x] `CompletionContext`
- [x] `CompletionStateView`
- [x] `CompletionCandidate`
- [x] `CompletionSet`
- [x] `CommandReadiness`

## Implementation Checklist

- [x] Inspect current runtime parser and interactive shell behavior.
- [x] Implement tolerant tokenization with cursor spans and quote state.
- [x] Support command/subcommand, option-name, option-value, positional, meta-command, shell-escape, and unknown contexts.
- [x] Support `--flag=value`, leading `harness`, global options, `--`, repeated options, cursor-in-middle edits, and unterminated quotes.
- [x] Implement static command, option, and static value completions from metadata.
- [x] Implement dynamic task ID completion through `CompletionStateView`.
- [x] Implement dynamic ticket ID completion through `CompletionStateView`.
- [x] Implement task-scoped ticket completion for `resume --ticket` and `supervise --ticket`.
- [x] Redact candidate display/detail text.
- [x] Return no harness completions for shell escapes and provide a hint row.
- [x] Implement readiness diagnostics for missing required positionals/options and conflicts.
- [x] Add bounded session cache with invalidation hooks or explicit TODO-compatible interface if cache wiring needs TUI state.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Root and nested command completion.
- [x] Option-name and option-value completion.
- [x] Task ID and ticket ID completion with fake state view.
- [x] Scoped ticket completion.
- [x] Cursor offsets and replacement spans.
- [x] Global options before command paths.
- [x] Leading `harness`.
- [x] `--flag=value`.
- [x] Unterminated quotes.
- [x] Dynamic query failure with static fallback.
- [x] Hint/loading/error/stale rows.
- [x] Redacted display text.
- [x] Fake state view proves no mutating/provider path is reachable.

## Acceptance Command

- [x] `cargo test completion`

## Review Checklist

- [x] Reviewer verifies completion does not call providers or mutating service methods.
- [x] Reviewer verifies command lists come from metadata.
- [x] Reviewer verifies replacement values are bare IDs/static values, not display labels.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review findings to remediate:

- [x] `CommandReadiness` excludes the token under the cursor when the cursor is at the end of that token, so fully typed commands can report missing values until trailing whitespace is added.
- [x] Interactive meta commands `exit`, `quit`, and `help` never become ready because readiness treats their `node == None` analysis as missing command.

Review notes:

- Initial review found two readiness defects.
- Remediation added readiness scanning for completed cursor-token values and exact meta-command readiness.
- Fresh review subagent passed with no findings.
- Local verification passed: `cargo fmt --check`, `cargo test completion`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
