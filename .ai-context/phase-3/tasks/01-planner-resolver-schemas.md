# Workstream A: Planner And Resolver Schemas, Prompting, Context Packing

Design reference: `.ai-context/phase-3/Design.md`, Workstream A.

## Scope

Implement strict remote planner and ticket resolver contracts, prompt builders, and deterministic context packing. Remote outputs remain untrusted until schema validation passes.

## Owned Files

- `src/planner/**`
- `src/prompts/**`

## Shared Files

- `src/domain/**` only to consume Workstream 0 types.
- `tests/support/**` only to consume fake contracts.
- `src/lib.rs` for the minimal `planner` module export.

## Blocked By

- Workstream 0.

## Interfaces Consumed

- Objective IDs/statuses and event contracts from Workstream 0.
- Existing prompt and provider request conventions.

## Interfaces Produced

- Strict planner response parser.
- Strict ticket resolver response parser.
- Planner and resolver request builders.
- Context packer with deterministic truncation manifest.

## Implementation Checklist

- [x] Define planner request and response structs with `schema_version`.
- [x] Enforce strict planner JSON parsing with unknown-field rejection.
- [x] Require stable unique `task_key` values.
- [x] Validate `depends_on` references against planner task keys.
- [x] Reject markdown/prose wrappers around planner JSON.
- [x] Enforce planner task count and size limits.
- [x] Define strict ticket resolver request and response structs.
- [x] Reject resolver responses containing patches, diffs, scripts, or executable shell guidance.
- [x] Build planner system prompt with harness role boundaries and output schema.
- [x] Build ticket resolver prompt with guidance-only constraints.
- [x] Implement deterministic context manifest generation.
- [x] Label all truncated context with byte counts and artifact hashes.

## Tests To Add

- [x] Valid planner schema parses.
- [x] Planner output rejects unknown fields, markdown, duplicate task keys, cycles, invalid dependencies, and path escapes.
- [x] Valid resolver schema parses.
- [x] Resolver output rejects patch-like, diff-like, and script-like content.
- [x] Context packing is deterministic for the same inputs.
- [x] Context packing records included and omitted sections.

## Acceptance Command

```sh
cargo test planner
cargo test resolver
cargo test context_pack
```

## Review Checklist

- [x] No remote output can directly mutate repo files.
- [x] No planner or resolver parser accepts loosely structured text.
- [x] Prompt builders clearly separate planner, resolver, and local worker roles.
- [x] Context packing does not leak unredacted secret-looking data.

## Done Criteria

- [x] Acceptance command passes.
- [x] Schema parsers are ready for service and monitor integration.
- [x] A separate review subagent passes this workstream.

## Review Findings

- 2026-05-14: Initial review failed because planner path validation allowed reserved/platform-dangerous paths, resolver validation missed bare numbered/bulleted executable command lists, and the `src/lib.rs` module export was not reflected in shared-file ownership.

## Remediation Notes

- 2026-05-14: Tightened lexical planner path validation for `.harness`, NUL, Windows drive/prefix, UNC, and home-prefix paths; rejected numbered/bulleted resolver command guidance; added tests; and documented `src/lib.rs` as a shared module-export file.
- 2026-05-14: Replacement review found backslash path bypasses and overly narrow executable-list detection. Normalized backslashes before path component checks and changed resolver list-item command detection to deny executable-looking list entries broadly.

## Review Notes

- 2026-05-14: Fresh review passed after the second remediation. Reviewer found no remaining blocking schema, prompt, or context-packing issues.
