# Workstream C2: Objective Runtime Dispatch And JSON Output

Design reference: `.ai-context/phase-3/Design.md`, Workstream C2.

## Scope

Wire parsed objective commands into runtime dispatch and enforce strict human/JSON output contracts.

## Owned Files

- `src/runtime/mod.rs`
- `src/cli/mod.rs`

## Shared Files

- `src/runtime/events.rs` only through Workstream 0 contracts.
- Service trait stubs from Workstream D if needed for compilation.

## Blocked By

- Workstream C1.
- Service trait stubs.

## Interfaces Consumed

- Objective command enum/parser.
- Objective event envelope.
- Objective service trait.

## Interfaces Produced

- Objective command dispatch.
- Strict stdout/stderr JSON contract.
- Exit code mapping.
- Human-readable output for objective commands.

## Implementation Checklist

- [x] Dispatch objective commands to objective service trait methods.
- [x] Implement `--output json` final-object stdout behavior.
- [x] Implement stderr NDJSON event envelope streaming.
- [x] Ensure JSON mode never mixes human prose into stderr.
- [x] Implement human output for objective start/list/get/plan/validate/supervise/cancel.
- [x] Map objective terminal states and errors to design exit codes.
- [ ] Preserve existing task/runtime output behavior.

## Tests To Add

- [x] Runtime unit tests for final JSON stdout.
- [x] Runtime unit tests for NDJSON event stderr.
- [x] Exit code mapping tests.
- [ ] Human output snapshot or predicate tests.
- [x] Phase 2 runtime regression tests.

## Acceptance Command

```sh
cargo test objective_runtime
cargo test json_output
cargo test exit_code
```

## Review Checklist

- [ ] JSON mode produces exactly one final JSON object on stdout.
- [ ] Every stderr line in JSON mode is an event envelope.
- [ ] Objective dispatch is behind service traits and can be faked in tests.
- [ ] Existing command behavior is not regressed.

## Done Criteria

- [x] Acceptance command passes.
- [ ] Binary e2e tests can drive objective commands.
- [x] A separate review subagent passes this workstream.

## Progress Log

- 2026-05-14: Added objective service trait methods and runtime option structs for start, supervise, and validate. Wired objective start/list/get/plan/validate/supervise/cancel dispatch through the service boundary, including streaming paths for start/supervise and objective JSON result helpers. Added fake-service runtime tests covering objective start event streaming, final JSON output, list/get/plan/validate/supervise/cancel dispatch, option propagation, and exit-code coverage. Verified with `cargo test objective_runtime`, `cargo test json_output`, and `cargo test exit_code`.
- 2026-05-14: Separate review passed with no findings for runtime JSON/event dispatch. Residual risks noted for later workstreams: default service methods still placeholder until D/E/F, and binary e2e coverage belongs to Workstream J.
