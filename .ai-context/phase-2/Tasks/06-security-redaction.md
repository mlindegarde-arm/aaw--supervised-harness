# Workstream 6: Security and Redaction

## Scope

Implement redaction, environment sanitization, provider URL policy, permission checks, and artifact safety helpers used before persistence, output, and provider requests.

## Owned Files

- [x] `src/security/**`

## Shared Files

- [x] None unless redactor trait changes are coordinated through Workstream 0 contracts.

## Interfaces Consumed

- [x] `Redactor` trait and error types from Workstream 0.
- [x] Config values from Workstream 2.

## Interfaces Produced

- [x] Redactor implementation.
- [x] High-confidence secret detection result types.
- [x] Environment sanitizer.
- [x] Provider URL policy validator.
- [x] Artifact permission and path safety helpers.

## Tests To Add

- [x] Redaction tests for auth headers, API keys, private keys, SSH keys, cookies, passwords, cloud credentials, and high-entropy tokens.
- [x] Tests proving high-confidence secrets block escalation.
- [x] Environment sanitizer tests for allowlist-only retention and token/secret/password/API key variables.
- [x] Provider URL policy tests for ARM proxy, HTTPS requirements, direct OpenAI default rejection, query/fragment rejection, API-key-like URL data rejection, and localhost fake allowances.
- [x] Permission warning/failure tests where platform-supported.

## Acceptance Command

- [x] `cargo test security`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.
- [x] Workstream 2.

## Implementation Checklist

- [ ] Run the same redactor before artifact writes, ticket prompts, provider requests, provider errors, human output, JSON output, and risky SQLite fields. Security-owned redactor is implemented; consumer wiring belongs to artifact/prompt/provider/output/state owners.
- [x] Redact bearer/basic auth headers.
- [x] Redact API keys, proxy tokens, private-key blocks, SSH keys, cookies, passwords, session tokens, `.env`-style secrets, cloud credentials, and high-entropy token-like strings.
- [x] Block escalation with exit code `30` when high-confidence secrets remain in ticket evidence.
- [x] Sanitize validation and shell escape environments with an allowlist.
- [x] Enforce HTTPS for credentialed real providers.
- [x] Allow default credentialed host `openai-api-proxy.geo.arm.com`.
- [x] Allow HTTP localhost only when `allow_untrusted_provider_url = true` for tests.
- [x] Provide helpers to exclude `.env*` including `.envrc`, SSH stores such as `.ssh`, private-key files, credential stores, and local config from context.

## Review Checklist

- [x] Confirm redaction is centralized and reusable.
- [ ] Confirm all downstream consumers invoke redaction before artifact writes, ticket prompts, provider requests, human output, JSON output, and risky SQLite fields. Security-owned helpers are ready; full wiring belongs to later integration workstreams.
- [ ] Confirm stdout/stderr and JSON output cannot leak obvious secrets once downstream output sinks wire the redactor.
- [x] Confirm URL policy blocks unsafe credentialed destinations, query/fragment data, and API-key-like URL path material.
- [x] Confirm tests cover both redaction and blocking behavior.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Security helpers are ready for providers, workspace, state, artifacts, prompts, and output sinks.
- [x] Reviewer has passed this workstream.
