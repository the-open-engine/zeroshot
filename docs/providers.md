# Providers

Zeroshot supports two provider shapes:

- CLI-backed providers that shell out to a full agent CLI
- One bundled `gateway` provider that wraps OpenAI-compatible model APIs with a
  Zeroshot-owned tool runner

## Supported Providers

| Provider | CLI         | Install                                    |
| -------- | ----------- | ------------------------------------------ |
| Claude   | Claude Code | `npm install -g @anthropic-ai/claude-code` |
| Codex    | Codex       | `npm install -g @openai/codex`             |
| Gateway  | Bundled     | No external CLI required                   |
| Gemini   | Gemini      | `npm install -g @google/gemini-cli`        |
| Opencode | Opencode    | See https://opencode.ai                    |

## Selecting a Provider

- List providers: `zeroshot providers`
- Set default: `zeroshot providers set-default <provider>`
- Configure levels: `zeroshot providers setup <provider>`
- Override per run: `zeroshot run ... --provider <provider>`
- Env override: `ZEROSHOT_PROVIDER=codex`

## Gateway Provider

Use `gateway` for OpenAI-compatible model endpoints such as OpenRouter,
Ollama, vLLM, or self-hosted gateways. These stay model configs behind one
provider engine; do not add them as standalone provider ids.

Required settings:

```json
{
  "providerSettings": {
    "gateway": {
      "baseUrl": "http://127.0.0.1:11434",
      "apiKey": "gateway-key",
      "model": "openrouter/meta-llama/test-model",
      "toolPolicy": {
        "roots": ["/absolute/path/to/worktree"],
        "commands": ["node"]
      }
    }
  }
}
```

Notes:

- `toolPolicy` is required. There is no default file or shell access.
- `headers` is optional for extra gateway-specific request headers.
- `model` may be any non-empty provider-specific model id.

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

Zeroshot does not inject credentials for external CLIs. When using `--docker`,
mount your provider config directories explicitly.

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

## Provider CLI Helper

Provider command construction, feature probing, model resolution, output
parsing, error classification, redaction metadata, and executable JSON behavior
live behind the strict TypeScript helper in `src/agent-cli-provider/`.

The public process contract is `zeroshot-agent-provider`, a JSON stdin/stdout
executable for provider-only commands: `probe`, `build-command`,
`parse-output`, `classify-error`, and `invoke`.

This helper does not share Zeroshot clusters, task store, message bus,
scheduler, PR/ship flow, TUI, or orchestration policy. Consumers such as
Orchestra must call the JSON executable contract and must not import Zeroshot
internals.

See `docs/provider-cli-helper.md` for the ownership boundary, non-goals, rollout
rules, and required verification commands.
