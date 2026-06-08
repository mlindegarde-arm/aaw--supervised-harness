# Workstream 5: Providers and Fakes

## Scope

Implement Ollama and OpenAI-compatible provider clients, provider error taxonomy mapping, retry policy, model readiness checks, and deterministic trait/HTTP fakes for tests.

## Owned Files

- [x] `src/providers/**`

## Shared Files

- [x] None unless provider trait changes are coordinated through Workstream 0 contracts.

## Interfaces Consumed

- [x] `ModelProvider`, `ModelRequest`, `ModelResponse`, and `ProviderError` from Workstream 0.
- [x] Config values from Workstream 2.
- [x] Provider URL policy from Workstream 6 when available.

## Interfaces Produced

- [x] Ollama provider client.
- [x] OpenAI-compatible Responses API provider client.
- [x] Provider readiness/model listing helpers.
- [x] Trait fake provider for unit tests.
- [x] Fake HTTP Ollama and OpenAI-compatible servers for integration tests.

## Tests To Add

- [x] Ollama request construction and response parsing tests.
- [x] OpenAI Responses request construction and output extraction tests.
- [x] Provider retry policy tests.
- [x] Provider error taxonomy tests.
- [x] Fake HTTP server tests for success and sequenced 5xx retry responses.
- [x] Fake HTTP server tests for malformed JSON, missing model, and 429.

## Acceptance Command

- [x] `cargo test providers`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.
- [x] Workstream 2.

## Implementation Checklist

- [x] Implement `GET {base_url}/api/tags` and `POST {base_url}/api/generate`.
- [x] Send Ollama generation with `stream:false`, `keep_alive`, and configured options.
- [x] Require Ollama `done = true` and extract `response`.
- [x] Implement OpenAI `GET {base_url}/models` and `POST {base_url}/responses`.
- [x] Normalize OpenAI `base_url` by trimming trailing slashes.
- [x] Send `stream:false`, `store:false`, metadata, and max output tokens.
- [x] Require OpenAI top-level `status = "completed"`.
- [x] Reject top-level errors, incomplete responses, refusal-only output, and empty output.
- [x] Extract only nested `output_text` items.
- [x] Preserve OpenAI top-level `id` as `response_id`.
- [x] Implement bounded retry for rate limit, timeout, and server errors only.
- [x] Ensure CI fake-provider tests never use real network URLs.

## Review Checklist

- [x] Confirm no silent model fallback is possible.
- [x] Confirm authorization is not forwarded across redirects.
- [x] Confirm API keys are not logged or persisted.
- [x] Confirm fake servers assert request shapes required by `design.md`.
- [x] Confirm real provider smoke remains separate from hermetic CI.

## Review Findings

- [x] Enforce credentialed provider URL policy for OpenAI-compatible clients.
- [x] Honor configured provider timeout and retry settings.
- [x] Use configured Ollama temperature when request temperature is absent.
- [x] Redact provider HTTP error bodies before exposing `ProviderError`.
- [x] Reject missing and non-string OpenAI response model fields.
- [x] Support fake HTTP delay/drop timeout simulation.
- [x] Make sequenced fake HTTP routes fail when exhausted instead of repeating success forever.
- [x] Make Ollama/OpenAI success helpers assert required request body/header shapes.
- [x] Require Ollama fake generation requests to include `system`.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Provider fakes are usable by orchestrator, doctor, and e2e tests.
- [x] Reviewer has passed this workstream.
