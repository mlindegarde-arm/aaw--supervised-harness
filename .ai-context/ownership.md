# Workstream File Ownership

This map follows `.ai-context/design.md` and `.ai-context/approach.md`. Subagents should stay inside their owned paths. Any change outside ownership must be recorded in the relevant task list and coordinated before editing.

| Workstream | Owns | Notes |
| --- | --- | --- |
| 0. Scaffold/shared contracts | `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/domain/**`, `src/error.rs`, module skeletons | Shared contract files. Later edits require coordination. |
| 1. CLI/runtime | `src/cli/**`, `src/runtime/**` | `src/main.rs` only for agreed entrypoint wiring. |
| 2. Config/filesystem | `src/config/**`, config path helper modules | May consume `HarnessConfig`; contract changes require coordination. |
| 3. State store | `src/state/**`, embedded migrations | Owns SQLite repository implementation and migrations. |
| 4. Workspace manager | `src/workspace/**` | Owns git, worktree, diff, validation command, and shell escape execution. |
| 5. Providers/fakes | `src/providers/**` | Owns Ollama/OpenAI clients and provider fakes. |
| 6. Security/redaction | `src/security/**` | Owns redaction, environment sanitizer, URL policy, and permission helpers. |
| 7. Prompt/patch contracts | `src/prompts/**`, `src/patch/**` | Owns prompt construction, response parsing, and patch safety. |
| 8. Orchestrator/service | `src/orchestrator/**`, `src/service/**` | Owns RWL loop and service use cases. |
| 9. Interactive shell | `src/interactive/**` | Owns line-editor shell and interactive sink. |
| 10. Doctor/diagnostics | `src/doctor/**` | Owns readiness diagnostics. |
| 11. E2E fixtures/tests | `tests/**`, `fixtures/**` | Owns hermetic fixtures, e2e tests, and manual smoke docs. |

## Shared Contract Rules

- `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/domain/**`, and `src/error.rs` are Phase 0 contract files.
- Parallel subagents must not edit shared contract files unless their task list explicitly records the required contract update.
- New module exports should be added by the owning workstream when possible.
- Cross-workstream behavior should be expressed through the traits created in Phase 0.

