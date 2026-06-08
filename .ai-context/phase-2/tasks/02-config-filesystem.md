# Workstream 2: Config and Filesystem

## Scope

Implement config loading/writing, repository discovery, `.harness/` initialization, filesystem permissions, and path helpers for state, artifacts, logs, and external worktree roots.

## Owned Files

- [x] `src/config/**`
- [x] Config-related path helper modules.

## Shared Files

- [x] `src/domain/config.rs` or equivalent only if Workstream 0 left config fields incomplete.

## Interfaces Consumed

- [x] `HarnessConfig` from Workstream 0.
- [x] Error/result types from Workstream 0.

## Interfaces Produced

- [x] Config loader with defaults from `design.md`.
- [x] Config writer for `.harness/config.toml`.
- [x] Repository root discovery.
- [x] State directory and artifact/log path helpers.
- [x] Worktree root normalization outside the source tree.

## Tests To Add

- [x] Unit tests for default config values.
- [x] Unit tests for env overrides used by tests.
- [x] Integration-style tests for `init` path creation in a git fixture.
- [x] Tests for failing outside a git repository.
- [x] Tests for worktree root normalization.

## Acceptance Command

- [x] `cargo test config`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.

## Implementation Checklist

- [x] Implement default `.harness/config.toml` shape exactly as in `design.md`.
- [x] Resolve repo root with `git rev-parse --show-toplevel` unless `--repo` is supplied.
- [x] Make `harness init` fail outside a git repository.
- [x] Create `.harness/`, logs, artifacts, config, and state parent paths.
- [x] Warn if `.harness/` is not ignored by git.
- [x] Ensure state directories use `0700` and files use `0600` where supported.
- [x] Normalize configured worktree root to an absolute path.
- [x] Enforce that worktrees are outside the source tree.
- [x] Support test env overrides from `design.md`.

## Review Checklist

- [x] Confirm config defaults match `design.md`.
- [x] Confirm API keys are never stored in config.
- [x] Confirm repo discovery failure behavior is deterministic.
- [x] Confirm path helpers prevent accidental worktree placement under repo root.
- [x] Confirm permissions are checked or set where supported.

## Review Findings

- [x] Fix double worktree-root namespacing after config normalization.
- [x] Reject worktree roots that resolve through symlinks into the repo.
- [x] Keep `ConfigPaths::config_file` fixed at `.harness/config.toml` even when `state_dir` is customized.

## Done Criteria

- [x] Acceptance commands pass.
- [x] `init` has enough behavior for state and CLI workstreams to consume.
- [x] Reviewer has passed this workstream.
