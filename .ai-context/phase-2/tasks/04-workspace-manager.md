# Workstream 4: Workspace Manager and Command Runner

## Scope

Implement git repository checks, external worktree lifecycle, command execution for validation and shell escapes, diff capture, cleanup, and worktree safety checks.

## Owned Files

- [x] `src/workspace/**`

## Shared Files

- [x] None unless coordinated through shared traits.

## Interfaces Consumed

- [x] `WorkspaceManager` and `CommandRunner` traits from Workstream 0.
- [x] Config/path helpers from Workstream 2.
- [x] Security environment sanitizer from Workstream 6 when available.

## Interfaces Produced

- [x] Git root, dirty status, base ref, and base commit APIs.
- [x] Worktree create/reuse/verify/remove APIs.
- [x] Validation command runner.
- [x] Shell escape runner for interactive mode.
- [x] Diff capture APIs.

## Tests To Add

- [x] Tests using temporary git repositories for root discovery and dirty-state checks.
- [x] Worktree create/reuse/verify/remove tests.
- [x] Command execution tests for stdout/stderr/exit code/duration/truncation.
- [x] Timeout and process group cleanup tests where feasible.
- [x] Cleanup refusal tests for dirty worktrees.

## Acceptance Command

- [x] `cargo test workspace`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.
- [x] Workstream 2.

## Implementation Checklist

- [x] Use `git worktree add -b harness/task_<task-id> <path> <base-ref-or-commit>`.
- [x] Store and verify worktree path, branch, base ref, base commit, and last seen HEAD.
- [x] Reuse only the recorded worktree path for a task.
- [x] Verify worktree registration with `git worktree list --porcelain`.
- [x] Refuse to reset or clean dirty worktrees automatically.
- [x] Implement source repo dirty check with `git status --porcelain=v1`.
- [x] Capture final and per-attempt diffs.
- [x] Run validation commands from the task worktree root.
- [x] Run shell escapes from the repo root, not the task worktree.
- [x] Capture command stdout/stderr, exit code, duration, timeout, and truncation metadata.
- [x] Remove worktrees through `git worktree remove`, with `--force` only when requested.

## Implementation Notes

- Extended `RecordedWorktree` in `src/workspace/mod.rs` with `base_ref` and `base_commit` so reuse/verification can return the complete `WorktreeInfo` contract without inventing missing base metadata.
- Routed workspace patch check/apply through `src/patch/mod.rs` safety validation, including path traversal, deletes, renames, binary patches, special modes, and file/byte limits.
- Command execution now clears the parent environment and applies `DefaultEnvironmentSanitizer` to caller-provided environment values before spawning shells.
- Diff capture now includes untracked new regular files when their generated diffs pass patch safety validation.
- Recorded worktree reuse now verifies that the registered branch is exactly `harness/task_<task-id>` for the requested task.
- Integrated `cargo fmt --check`, `cargo test workspace`, and `cargo test` pass.

## Review Checklist

- [x] Confirm worktrees are never created inside the source tree.
- [x] Confirm dirty worktrees are not reset or cleaned automatically.
- [x] Confirm commands run with non-interactive stdin and output limits.
- [x] Confirm cleanup refuses unrecorded dirty changes unless forced.
- [x] Confirm tests do not depend on global git config beyond basic availability.

## Done Criteria

- [x] Acceptance commands pass.
- [x] Workspace APIs support orchestrator and doctor needs.
- [x] Reviewer has passed this workstream.
