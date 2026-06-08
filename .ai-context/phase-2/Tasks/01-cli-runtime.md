# Workstream 1: CLI and Runtime

## Scope

Implement the one-shot `clap` command tree and non-exiting `CommandRuntime` used by both CLI and interactive shell. Rendering must go through output sinks and JSON mode must emit exactly one final JSON object on stdout.

## Owned Files

- [x] `src/cli/**`
- [x] `src/runtime/**`
- [x] `Cargo.toml` for the required `clap` dependency.

## Shared Files

- [x] `src/main.rs` only for wiring `CommandRuntime` return values to process exit in `main()`.
- [x] `src/lib.rs` only for module exports if Phase 0 did not already expose the needed modules.

## Interfaces Consumed

- [x] `CommandExit`, `CommandResult`, `CommandEvent`, and `OutputSink` from Workstream 0.
- [x] `HarnessService` trait from Workstream 0.
- [x] Config and repo path options from Workstream 2.

## Interfaces Produced

- [x] `build_cli()` or equivalent single command catalog.
- [x] `CommandRuntime` that uses non-exiting parse APIs.
- [x] Human, JSON, and interactive-compatible sink implementations.
- [x] Command handlers for the MVP command surface that dispatch to service methods.

## Tests To Add

- [x] Parser tests for all documented commands and global flags.
- [x] Tests proving help/version/parse errors return `CommandExit` instead of exiting.
- [x] Tests proving repeated command execution works in one process.
- [x] JSON output tests proving stdout contains one final object.

## Acceptance Command

- [x] `cargo test cli`
- [x] `cargo test runtime`
- [x] `cargo test`

## Blocked By

- [x] Workstream 0.
- [x] Workstream 2 for config path option semantics.

## Implementation Checklist

- [x] Build the documented command surface using one introspectable command catalog.
- [x] Support global flags: `--output`, `--quiet`, `--repo`, and `--state-dir`.
- [x] Strip an optional leading `harness` token for interactive dispatch.
- [x] Use shell-like tokenization for runtime input.
- [x] Return exit code `2` for parse or usage errors.
- [x] Route all rendering through output sinks.
- [x] Ensure service methods never write directly to stdout/stderr.
- [x] Make JSON mode emit exactly one final JSON object on stdout.
- [x] Send long-running progress to stderr in JSON mode.
- [x] Wire `main()` as the only place that converts `CommandExit` to process exit.

## Contract Changes Needed

- [x] Add `clap` to `Cargo.toml` and back the documented command surface with `build_clap()`. Runtime dispatch still uses the non-exiting parser so parse/help/version errors continue returning `CommandExit`, and tests keep the manual parser and clap tree aligned.
- [x] `cargo test cli runtime` is not a valid Cargo invocation (`runtime` is rejected as an unexpected argument). Workstream 1 verified the equivalent filters with `cargo test cli` and `cargo test runtime`.
- [x] Interactive shell behavior is currently implemented in `src/cli/mod.rs`; future Workstream 9 may move this behind `src/interactive` without changing behavior.

## Review Checklist

- [x] Confirm no `std::process::exit` occurs outside `main()`.
- [x] Confirm command names and flags match `design.md`.
- [x] Confirm validation command arguments preserve quoted full commands.
- [x] Confirm JSON output is machine-safe and free of human prose.
- [x] Confirm shell-like tokenization is not whitespace splitting.

## Done Criteria

- [x] Acceptance commands pass.
- [x] CLI/runtime tests cover repeated in-process execution.
- [x] Reviewer has passed this workstream.
