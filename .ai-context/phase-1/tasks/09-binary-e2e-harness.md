# Workstream F0: Binary E2E Harness

Design reference: `.ai-context/phase-2-design.md` sections `End-to-End Test Strategy`, `Binary Fake Provider Contract`, and `Early Binary Acceptance`.

## Scope

- [x] Build reusable actual-binary e2e test support.
- [x] Add fake provider servers and request recording.
- [x] Land pending/ignored supervise acceptance skeletons.

## Owned Files

- [x] `tests/support/binary.rs`
- [x] `tests/support/fake_providers.rs`
- [x] `tests/support/fixtures.rs`
- [x] `tests/e2e.rs` skeleton wiring

## Shared Files

- [x] Coordinate with F1 before changing support APIs after F1 starts.
- [x] Do not implement supervisor behavior here.

## Blocked By

- [x] Workstream A command parsing exposes `supervise` surface.

## Interfaces Consumed

- [x] CLI binary path and current e2e patterns.
- [x] Config file format.
- [x] Provider client HTTP behavior.

## Interfaces Produced

- [x] Binary runner with stdout/stderr/exit-code capture.
- [x] Fixture repo builder.
- [x] Fake provider config injection.
- [x] Fake HTTP provider request recorder.
- [x] Outbound guard where feasible.

## Implementation Checklist

- [x] Inspect current e2e tests and test fixtures.
- [x] Add helper to build/run compiled `harness` binary.
- [x] Add temp git repo fixture with deterministic failing/passing Rust project.
- [x] Add helper that runs `harness --repo <repo> init`.
- [x] Add helper to rewrite `.harness/config.toml` for local fake providers.
- [x] Add fake local Ollama-compatible server.
- [x] Add fake OpenAI-compatible server.
- [x] Add request recording and ordinal response scripting.
- [x] Add guard/assertion that no real provider URL is used.
- [x] Sanitize inherited provider override env vars for binary fake-provider runs.
- [x] Add ignored/pending supervise skeletons for F1 scenarios.
- [x] Run formatter.
- [x] Run the acceptance command.
- [x] Update this checklist as items complete.

## Tests To Add

- [x] Binary runner captures stdout, stderr, and exit code.
- [x] Fixture repo initializes successfully.
- [x] Fake provider receives and records a request.
- [x] Config injection points providers at local fake servers.
- [x] Guard fails test on unexpected outbound provider host where feasible.
- [x] Inherited provider override env vars cannot redirect binary fake-provider runs.

## Acceptance Command

- [x] `cargo test --test e2e binary_harness`

## Review Checklist

- [x] Reviewer verifies tests drive the compiled CLI binary.
- [x] Reviewer verifies fake providers are deterministic and record requests.
- [x] Reviewer verifies no real provider URLs are reachable from e2e.
- [x] Reviewer verifies acceptance command passes or any blocker is documented.

Review finding remediated:

- [x] Binary e2e harness inherits provider override env vars (`HARNESS_OLLAMA_BASE_URL`, `HARNESS_OPENAI_BASE_URL`, related API/model/env overrides), so fake-provider tests can be redirected to real local/provider endpoints.

Review notes:

- Initial review found inherited provider override env vars could redirect fake-provider tests.
- Remediation made `BinaryHarness` clear/rebuild a sanitized environment and added a poisoned-env regression test.
- Fresh review subagent passed with no findings.
- Local verification passed: `cargo fmt --check`, `cargo test --test e2e binary_harness`, and full `cargo test`.

## Done Criteria

- [x] Implementation checklist complete.
- [x] Acceptance command passes.
- [x] A separate review subagent has passed this workstream.
- [x] Any requested review fixes are implemented and re-reviewed until pass.
