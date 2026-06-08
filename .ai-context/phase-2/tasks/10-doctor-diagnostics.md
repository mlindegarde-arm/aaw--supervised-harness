# Workstream 10: Doctor and Diagnostics

## Scope

Implement offline and provider readiness diagnostics for repository setup, config, SQLite state, git/worktree capabilities, security policy, Ollama, and OpenAI-compatible provider access.

## Owned Files

- [x] `src/doctor/**`

## Shared Files

- [x] CLI command handler files only for wiring the `doctor` subcommand if Workstream 1 left placeholders.

## Interfaces Consumed

- [x] Config/filesystem from Workstream 2.
- [x] Workspace manager from Workstream 4.
- [x] Providers from Workstream 5.
- [x] Security URL policy and permission checks from Workstream 6.
- [x] CLI/runtime output contracts from Workstream 1.

## Interfaces Produced

- [x] `doctor --offline`.
- [x] `doctor --providers local|all`.
- [x] `doctor --deep`.
- [x] Human and JSON diagnostic result structures.

## Tests To Add

- [x] Offline doctor tests in valid and invalid fixture repos.
- [x] Provider-local doctor tests against fake Ollama.
- [x] Provider-all doctor tests against fake Ollama and fake OpenAI-compatible server.
- [x] Deep doctor tests for tiny generation request using fakes.
- [x] Exit code `20` tests for dependency/provider readiness failures.

## Acceptance Command

- [x] `cargo test doctor`
- [x] `cargo test`

## Blocked By

- [x] Workstream 2.
- [x] Workstream 4.
- [x] Workstream 5.
- [x] Workstream 6.

## Implementation Checklist

- [x] Check repository discovery and `.harness/` readiness.
- [x] Check config parseability and effective provider settings.
- [x] Check SQLite state path and permissions.
- [x] Check git availability and worktree support.
- [x] Check security policy for provider URLs.
- [x] Implement offline mode without network calls.
- [x] Implement local provider checks against Ollama model listing.
- [x] Implement all provider checks against Ollama and OpenAI model listing.
- [x] Implement deep checks with tiny fake-compatible generation requests.
- [x] Redact provider failures before output.
- [x] Return exit code `20` for readiness failures.

## Review Checklist

- [x] Confirm `--offline` performs no network requests.
- [x] Confirm diagnostics are actionable in human mode.
- [x] Confirm JSON diagnostics are structured and stable.
- [x] Confirm provider errors are redacted.
- [x] Confirm fake providers are used in tests.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Doctor supports required manual smoke prerequisites.
- [x] Reviewer has passed this workstream.

Note: `cargo test doctor` and the full `cargo test` suite pass.
