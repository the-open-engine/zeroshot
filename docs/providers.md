# Providers

Zeroshot shells out to provider CLIs. It does not store API keys or manage
authentication. Use each CLI's login flow or API key setup.

## Supported Providers

| Provider | CLI         | Install                                    |
| -------- | ----------- | ------------------------------------------ |
| Claude   | Claude Code | `npm install -g @anthropic-ai/claude-code` |
| Codex    | Codex       | `npm install -g @openai/codex`             |
| Gemini   | Gemini      | `npm install -g @google/gemini-cli`        |
| Opencode | Opencode    | See https://opencode.ai                    |

## Selecting a Provider

- List providers: `zeroshot providers`
- Set default: `zeroshot providers set-default <provider>`
- Configure levels: `zeroshot providers setup <provider>`
- Override per run: `zeroshot run ... --provider <provider>`
- Env override: `ZEROSHOT_PROVIDER=codex`

## Model Levels

Zeroshot uses provider-agnostic levels:

- `level1`: cheapest/fastest
- `level2`: default
- `level3`: most capable

Set levels per provider in settings:

```json
{
  "providerSettings": {
    "codex": {
      "minLevel": "level1",
      "maxLevel": "level3",
      "defaultLevel": "level2",
      "levelOverrides": {
        "level1": { "model": "codex-model-main", "reasoningEffort": "low" },
        "level3": { "model": "codex-model-main", "reasoningEffort": "xhigh" }
      }
    }
  }
}
```

Notes:

- `reasoningEffort` applies to Codex and Opencode only.
- `model` is still supported as a provider-specific escape hatch.

## Docker Isolation and Credentials

Zeroshot does not inject credentials for non-Claude CLIs. When using
`--docker`, mount your provider config directories explicitly.

Examples:

```bash
# Codex
zeroshot run 123 --docker --mount ~/.config/codex:/home/node/.config/codex:ro

# Gemini (use gemini or gcloud config as needed)
zeroshot run 123 --docker --mount ~/.config/gemini:/home/node/.config/gemini:ro
zeroshot run 123 --docker --mount ~/.config/gcloud:/home/node/.config/gcloud:ro
```

Mount presets in `dockerMounts` include: `codex`, `gemini`, `gcloud`, `claude`, `opencode`.

Use `--no-mounts` to disable all credential mounts (you will get a warning if
credentials are missing).

## GitHub Copilot CLI

The `copilot` provider integrates the GitHub Copilot CLI.

Install:

```bash
npm install -g @github/copilot
```

Authenticate (interactive, one-time):

```bash
copilot
# then inside the REPL:
/login
```

Credentials are stored under `~/.copilot/`. Logs are under `~/.copilot/logs/`.

Usage with zeroshot:

```bash
zeroshot run 123 --provider copilot
```

Models (level mapping):

- `level1` → `gpt-5-mini`
- `level2` → `claude-sonnet-4.5` (default)
- `level3` → `claude-opus-4.6`

Override per level via `providerSettings.copilot.levelOverrides`.

Limitations:

- Copilot CLI emits **plain text** with `--silent` (no structured streaming
  JSON like Claude/Codex). Token usage is not reported, and live tool-call
  events are not surfaced. The whole stdout stream is treated as text.
- `jsonSchema` is supported only by **prompt-injecting** the schema (no native
  `--output-schema` flag); reliability depends on the underlying model.
- MCP servers, thinking mode, and reasoningEffort are not supported.
- Auto-approval is enabled via `--allow-all` (a.k.a. `--yolo`); use isolation
  (`--worktree` / `--docker`) when running untrusted prompts.
