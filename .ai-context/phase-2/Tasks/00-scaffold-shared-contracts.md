# Workstream 0: Scaffold and Shared Contracts

## Scope

Create the Rust project scaffold, compileable module tree, shared domain types, error types, runtime result types, provider contracts, service/store/workspace traits, and file ownership map required before parallel implementation begins.

## Owned Files

- [x] `Cargo.toml`
- [x] `src/main.rs`
- [x] `src/lib.rs`
- [x] `src/domain/**`
- [x] `src/error.rs`
- [x] Module placeholder files for all workstreams.
- [x] `.ai-context/ownership.md`

## Shared Files

- [x] `Cargo.toml`
- [x] `src/lib.rs`

Shared files must only be changed here unless a later workstream records and coordinates a contract update.

## Interfaces Consumed

- [x] `design.md` Shared Contracts Required Before Parallel Work.
- [x] `design.md` State Store Contract.
- [x] `design.md` Provider Contracts.
- [x] `design.md` CommandRuntime Contract.

## Interfaces Produced

- [x] ID newtypes or validated string wrappers for `task_`, `run_`, `att_`, `ticket_`, `res_`, `art_`, and `event_`.
- [x] Status enums for task, run, attempt, and ticket state.
- [x] `HarnessError` and `HarnessResult<T>`.
- [x] `CommandExit`, `CommandResult`, `CommandEvent`, and `OutputSink`.
- [x] `HarnessConfig` top-level shape with nested config structs.
- [x] `ModelRequest`, `ModelResponse`, and `ProviderError`.
- [x] Domain structs: `Task`, `Run`, `Attempt`, `Ticket`, `TicketResolution`, `Artifact`.
- [x] Traits: `HarnessService`, `TaskStore`, `WorkspaceManager`, `ModelProvider`, `CommandRunner`, `Redactor`.

## Tests To Add

- [x] Unit tests for ID parsing/format validation.
- [x] Unit tests for status enum serialization names.
- [x] Unit tests proving `CommandExit` maps to the documented exit codes.

## Acceptance Command

- [x] `cargo fmt --check`
- [x] `cargo test`

## Blocked By

- [ ] None.

## Implementation Checklist

- [x] Initialize the Rust crate with one `harness` binary.
- [x] Add dependencies needed by shared contracts only.
- [x] Create the module tree named in `design.md`.
- [x] Define domain structs with fields matching the MVP schema and contracts.
- [x] Define status enums and explicit string serialization values.
- [x] Define the shared error/result types without provider- or store-specific implementation detail.
- [x] Define command runtime result/event/output abstractions without printing from service traits.
- [x] Define provider request/response structs and provider error taxonomy.
- [x] Define trait signatures with async boundaries where required by `design.md`.
- [x] Add file ownership map at `.ai-context/ownership.md`.
- [x] Keep placeholder modules compileable without implementing downstream behavior.

## Review Checklist

- [x] Confirm the project compiles and tests run.
- [x] Confirm all required contracts from Phase 0 exist.
- [x] Confirm no downstream workstream behavior was implemented prematurely.
- [x] Confirm ownership map matches `design.md` Parallel Workstream Matrix.
- [x] Confirm no process exit occurs outside `main()`.

## Review Findings

- [x] Fix ID parsing so prefixed IDs require a valid 26-character ULID suffix.
- [x] Expand `TaskStore` contract for validation commands, attempts, artifacts, events, ticket resolutions, idempotent ticket creation, status transitions, and lease recovery.
- [x] Expand workspace contracts for base refs, recorded worktree reuse, patch check/apply, and command execution options.
- [x] Change Ollama temperature config from string to numeric `f32`.
- [x] Add ticket-resolution retrieval scoped by ticket/run/resolution ID for explicit resume and ticket inspection.
- [x] Add run lifecycle update contract for phase, finish time, final diff path, and last error.
- [x] Add command execution contract fields for configured shell path, stdin mode, and process-group timeout handling.
- [x] Add command execution defaults to `HarnessConfig`.
- [x] Add per-run repo root, base ref, base commit, and dirty-state summary metadata.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Shared interfaces are stable enough for Layer 1 workstreams.
- [x] Reviewer has passed this workstream.
