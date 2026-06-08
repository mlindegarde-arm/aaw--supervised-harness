# Workstream 11: E2E Fixtures and Acceptance Tests

## Scope

Build hermetic end-to-end fixture repositories, fake-provider acceptance tests, final MVP acceptance tests, and manual real-provider smoke documentation.

## Owned Files

- [x] `tests/**`
- [x] `fixtures/**`
- [x] Acceptance scripts or test utilities under test-owned paths.
- [x] Manual smoke documentation if stored outside source modules.

## Shared Files

- [x] None unless test helpers require coordinated exports.

## Interfaces Consumed

- [x] CLI/runtime from Workstream 1.
- [x] Config/filesystem from Workstream 2.
- [x] State store from Workstream 3.
- [x] Workspace manager from Workstream 4.
- [x] Providers/fakes from Workstream 5.
- [x] Security/redaction from Workstream 6.
- [x] Prompt/patch contracts from Workstream 7.
- [x] Orchestrator/service from Workstream 8.
- [x] Interactive shell from Workstream 9 where applicable.
- [x] Doctor from Workstream 10.

## Interfaces Produced

- [x] Fixture repository creation helpers.
- [x] Hermetic e2e tests.
- [x] Acceptance assertions for state rows, artifacts, manifests, provider requests, and exit codes.
- [x] Manual real-provider smoke instructions.

## Tests To Add

- [x] `fixtures/rust_success`.
- [x] `fixtures/rust_validation_fails_then_stuck`.
- [x] `fixtures/rust_resume_after_ticket`.
- [x] `fixtures/not_git_repo`.
- [x] End-to-end tests for `init`, offline doctor, task create, task run success, stuck ticket, ticket resolve, resume, redaction, and patch safety.

## Acceptance Command

- [x] `cargo test`
- [x] `cargo test --test fixture_skeleton`
- [x] `cargo test --test e2e`

## Blocked By

- [x] Workstream 1 through Workstream 10 progressively.

## Implementation Checklist

- [x] Create git-initialized fixture repos during tests.
- [x] Keep generated fixture validation/build output from dirtying fixture repos.
- [x] Ensure fake providers are used for all CI e2e tests.
- [x] Assert `harness init --output json` creates `.harness/state.sqlite` and default config.
- [x] Assert current `init_repo` API creates `.harness/state.sqlite` after state store open and default config.
- [x] Assert `doctor --offline --output json` exits `0` in a valid fixture repo.
- [x] Assert `task create` returns a `task_id` and persists validation commands.
- [x] Assert success fixture completes with exit code `0`.
- [x] Assert validation failure fixture exits `10` and returns `ticket_id`.
- [x] Assert fake OpenAI ticket resolution writes a redacted resolution artifact and response ID.
- [x] Assert resume consumes the ticket resolution and completes the resume fixture.
- [x] Assert SQLite rows, artifact existence, manifest hashes, worktree path, and provider request content.
- [x] Assert secrets do not appear in stdout, stderr, SQLite fields, artifacts, or provider requests.
- [x] Assert patch safety rejection rules through end-to-end or integration tests.
- [x] Add manual real-provider smoke instructions using `ARM_OPENAI_API_KEY`.

## Review Checklist

- [x] Confirm tests are hermetic and never hit real provider URLs.
- [x] Confirm fixture setup is deterministic and isolated.
- [x] Confirm acceptance tests cover the final MVP criteria in `design.md`.
- [x] Confirm manual smoke is separate from CI.
- [x] Confirm failures produce useful diagnostics.

## Review Findings

- [x] Ignore fixture `target/` and `Cargo.lock` outputs so validation does not dirty source repos.
- [x] Prove fixtures compile with `cargo test --no-run` before asserting validation failure.
- [x] Add scenario metadata that distinguishes success, stuck-ticket, and resume-after-ticket fixtures.
- [x] `tests/e2e.rs` uses only temp fixtures and `FakeModelProvider`; no real provider URL is reachable from CI.
- [x] Production `harness init --output json` is wired through runtime and covered by CLI-binary e2e acceptance.
- [x] Production `DefaultHarnessService`/`RunOrchestrator` success path is covered with isolated external worktree, persisted `manifest.json`, and real SHA-256 artifact assertions.
- [x] Redaction coverage checks stdout, stderr, SQLite bytes, all artifact files, and all fake provider requests including the local provider.
- [x] Patch safety acceptance covers absolute paths, `.git/hooks`, symlink escape, deletes, renames, binary patches, and oversized patches.

## Done Criteria

- [x] `cargo test` passes.
- [x] `cargo test --test e2e` passes.
- [x] Final MVP acceptance criteria from `design.md` are covered by CLI init/doctor, task create/run, stuck ticket, ticket resolve, resume, artifact/manifest, redaction, and patch-safety acceptance.
- [x] Reviewer has passed this workstream.
