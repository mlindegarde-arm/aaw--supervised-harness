# Workstream 7: Prompt and Patch Contracts

## Scope

Implement prompt builders, evidence delimiters and budgets, Ollama patch/STUCK response parsers, OpenAI ticket prompt builders, patch safety validation, and artifact manifest helpers.

## Owned Files

- [x] `src/prompts/**`
- [x] `src/patch/**`

## Shared Files

- [x] None unless prompt/patch result types require coordinated domain updates.

## Interfaces Consumed

- [x] Domain and provider types from Workstream 0.
- [x] Redactor and secret blocking from Workstream 6.

## Interfaces Produced

- [x] Ollama worker prompt builder.
- [x] Ticket prompt builder.
- [x] Strict Ollama response parser.
- [x] Strict STUCK parser.
- [x] Patch validator and applier safety checks.
- [x] Artifact manifest record builder.

## Tests To Add

- [x] Prompt delimiter tests proving evidence is labeled as untrusted.
- [x] Prompt budget/truncation tests.
- [x] Parser tests for valid diff and valid STUCK responses.
- [x] Parser rejection tests for prose, multiple fences, nested fences, non-diff fences, missing STUCK fields, and multiline STUCK fields.
- [x] Patch safety tests for traversal, absolute paths, `.git/**`, `.harness/**`, hooks/config, symlink escapes, binary patches, mode-only patches, renames, deletes, oversized patches, and too many files.

## Acceptance Command

- [x] `cargo test prompts`
- [x] `cargo test patch`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.
- [x] Workstream 6.

## Implementation Checklist

- [x] Include the prompt safety contract in system prompts.
- [x] Delimit all repo files, diffs, logs, responses, and ticket resolutions as untrusted evidence with labels and byte counts.
- [x] Enforce prompt budgets from `design.md`.
- [x] Use deterministic head/tail truncation with byte-count markers.
- [x] Create provider-limit stuck tickets or errors when required evidence cannot fit.
- [x] Parse exactly one valid Ollama response shape: diff fence or STUCK block.
- [x] Reject any prose before or after the allowed response shape.
- [x] Run `git apply --check` before `git apply`.
- [x] Reject unsafe patch paths and unsupported patch operations.
- [x] Allow only new files and modifications to existing normal files.
- [x] Capture apply-check and apply stderr as artifacts through downstream interfaces.

## Implementation Notes

- Patch validation returns ordered `git apply --check -` and `git apply -` invocation contracts for the workspace layer to execute and persist stderr artifacts.
- No shared/domain/workspace/security contract edits were made.
- `cargo test prompts` and `cargo test patch` pass as the focused equivalent to the invalid two-filter Cargo command.

## Review Checklist

- [x] Confirm prompt injection protections are present in every prompt builder.
- [x] Confirm response parsing is strict and deterministic.
- [x] Confirm patch validation prevents filesystem escape.
- [x] Confirm OpenAI resolutions remain advisory only.
- [x] Confirm patch safety tests cover all MVP rejection rules.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Orchestrator can consume prompt and patch APIs.
- [x] Reviewer has passed this workstream.
