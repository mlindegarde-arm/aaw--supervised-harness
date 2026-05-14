# Manual Real-Provider Smoke

These checks are intentionally manual and must not run in CI. CI e2e tests use only local fake providers and the compiled harness binary.

Prerequisites:

- A disposable git repository with no uncommitted user work.
- A local Ollama-compatible coder model configured in `.harness/config.toml`.
- `ARM_OPENAI_API_KEY` exported for the OpenAI-compatible escalation provider.

Suggested smoke flow once production service wiring is available:

```sh
export ARM_OPENAI_API_KEY=...
harness --repo /path/to/disposable/repo init --output json
harness --repo /path/to/disposable/repo doctor --offline --output json
harness --repo /path/to/disposable/repo doctor --providers local --output json
harness --repo /path/to/disposable/repo task create \
  --title "Fix failing validation" \
  --goal "Make the failing fixture test pass without leaking secrets" \
  --validation "cargo test" \
  --output json
harness --repo /path/to/disposable/repo task run <task-id> --max-attempts 1 --output json
harness --repo /path/to/disposable/repo ticket resolve <ticket-id> --output json
harness --repo /path/to/disposable/repo resume <task-id> --ticket <ticket-id> --max-attempts 1 --output json
harness --repo /path/to/disposable/repo supervise <task-id> --max-attempts 1 --max-cycles 1 --output json
harness --repo /path/to/disposable/repo supervise \
  --create \
  --title "Fix failing validation through supervisor" \
  --goal "Make the failing fixture test pass without leaking secrets" \
  --validation "cargo test" \
  --max-attempts 1 \
  --max-cycles 1 \
  --output json
```

Manual acceptance:

- Real-provider commands are run only from a disposable repo.
- `--output json` writes exactly one final JSON object to stdout, and supervisor progress on stderr is newline-delimited JSON with chronological `inspect`, `run`, `resolve`, `resume`, and terminal phases as applicable.
- The process exit code equals the final JSON `exit_code`; nonzero supervisor results include nonempty `data.next_commands`.
- `.harness/state.sqlite`, logs, artifacts, and worktree paths are created under configured harness paths.
- Provider prompts, SQLite rows, stdout, stderr, logs, and artifacts do not contain API keys or bearer tokens.
- Completed supervisor runs leave the task `complete`, persist run artifacts and manifests, and consume ticket resolutions.
- A stuck/capped supervisor run exits with code `10` and does not make an extra OpenAI-compatible provider call after the configured cycle cap.
- Missing escalation-provider readiness exits with code `20`; provider URL policy blocks exit with code `30`.
