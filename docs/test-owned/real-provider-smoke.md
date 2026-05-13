# Manual Real-Provider Smoke

These checks are intentionally manual and must not run in CI. CI e2e tests use only in-process fake providers.

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
```

Manual acceptance:

- Real-provider commands are run only from a disposable repo.
- `.harness/state.sqlite`, logs, artifacts, and worktree paths are created under configured harness paths.
- Provider prompts, SQLite rows, stdout, stderr, logs, and artifacts do not contain API keys or bearer tokens.
- A stuck run exits with code `10`, ticket resolution records the provider response id, and resume consumes the resolution.
