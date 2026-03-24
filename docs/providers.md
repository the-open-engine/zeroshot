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
| MiniMax  | SDK-based   | `export MINIMAX_API_KEY=your-key`          |

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

## MiniMax (SDK-Based Provider)

MiniMax is the first SDK-based provider — it uses the MiniMax OpenAI-compatible API
directly instead of shelling out to an external CLI. Set your API key and you're ready:

```bash
export MINIMAX_API_KEY=your-api-key
zeroshot providers set-default minimax
```

Available models:

| Model                 | Context   | Best For                         |
| --------------------- | --------- | -------------------------------- |
| MiniMax-M2.7          | 1,000,000 | Complex tasks (default level2/3) |
| MiniMax-M2.7-highspeed| 1,000,000 | Faster responses                 |
| MiniMax-M2.5          | 204,000   | Balanced quality/cost            |
| MiniMax-M2.5-highspeed| 204,000   | Cheapest/fastest (level1)        |

MiniMax also implements the SDK extension point (`callSimple`/`callSDK`), enabling
features like output reformatting when other providers add SDK support.

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
