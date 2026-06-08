# Workstream G1: Validation Command Safety Policy

Design reference: `.ai-context/phase-3/Design.md`, Workstream G1.

## Scope

Implement validation command classification and repository path validation for planner-controlled commands and paths.

## Owned Files

- `src/security/**`
- `src/objective/validation_policy.rs` if an objective module is introduced
- Tests for validation command classification

## Shared Files

- `src/domain/**` only to consume Workstream 0 types.
- `src/planner/**` only to consume Workstream A command/path structs.

## Blocked By

- Workstream 0.
- Workstream A.

## Interfaces Consumed

- Planner validation command representation.
- Objective and validation review status contracts.

## Interfaces Produced

- `ValidationCommandPolicy`.
- `Trusted`, `NeedsReview`, and `Rejected` classification.
- `RepoPath` validator for planner-controlled paths.

## Implementation Checklist

- [x] Define validation command classification API.
- [x] Classify safe argv-style commands as `trusted`.
- [x] Classify ambiguous but non-destructive commands as `needs_review`.
- [x] Reject shell metacharacters, pipes, redirects, env assignment, `sh -c`, destructive commands, network tools, scripts, absolute paths, and unsafe repo writes.
- [x] Implement repo path validation for relative in-repo paths only.
- [x] Reject `..`, NUL, absolute paths, symlink escapes, `.git`, and unapproved `.harness` paths.
- [x] Ensure unsafe validation commands never become executable task validation rows.
- [x] Return actionable review reasons for non-trusted commands.

## Tests To Add

- [x] Trusted command matrix.
- [x] Needs-review command matrix.
- [x] Rejected command matrix.
- [x] Repo path validation matrix.
- [x] Regression tests for shell metacharacters and destructive commands.

## Acceptance Command

```sh
cargo test validation_command_policy
cargo test repo_path
```

## Review Checklist

- [x] Policy defaults to deny when uncertain.
- [x] Classification preserves reasons for user/TUI display.
- [x] The implementation avoids ad hoc string parsing where structured parsing is available.
- [x] Planner-controlled values cannot escape the repository.

## Done Criteria

- [x] Acceptance command passes.
- [ ] Service workstream can use the policy to gate generated validation commands.
- [x] A separate review subagent passes this workstream.

## Work Log

- 2026-05-14: Added `src/security/validation_command_policy.rs` with `ValidationCommandPolicy`, trusted/needs-review/rejected classification, trusted-only executable argv exposure, actionable reasons, and `RepoPath` validation including symlink escape checks. Added command and repo path test matrices. Acceptance commands are currently blocked by an unrelated incomplete `ObjectiveStore` impl in `src/state/objective_queries.rs`.
- 2026-05-14: First review found three safety gaps: unsafe paths hidden inside `--flag=value`, `.harness` command arguments not rejected by the classifier, and planner-owned paths still using a separate ad hoc validator. Remediated by inspecting `--flag=value` payloads, rejecting `.harness` in command arguments, exposing shared `RepoPath::validate_lexical`, using it from planner path validation, and adding repo-root planner validation for symlink escape checks.
- 2026-05-14: Verified with `cargo test validation_command_policy`, `cargo test repo_path`, and `cargo test planner`.
- 2026-05-14: Second review found two bypasses: repo-relative executables such as `./cargo` could be trusted by basename, and `cargo clippy --fix` was incorrectly trusted. Remediated by rejecting program path arguments and mutating cargo fix flags/subcommands. Verified with `cargo test validation_command_policy`.
- 2026-05-14: Re-review passed with no findings. Residual integration note: future planner ingestion must call the repo-aware validator before accepting planner-owned paths.
