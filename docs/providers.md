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
| Novita   | Codex       | `npm install -g @openai/codex`             |

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

## Novita Provider

Novita uses Codex CLI with an OpenAI-compatible API endpoint.

**API Key Setup:**

```bash
export NOVITA_API_KEY="your-novita-api-key"
```

**Model IDs (using `/` separator):**

- `deepseek/deepseek-v3.2` (default for level1/level2)
- `zai-org/glm-5` (default for level3)
- `minimax/minimax-m2.5`

**Docker Isolation:**
Novita uses Codex CLI credentials, so Docker needs the same mounts:

```bash
zeroshot run 123 --docker --mount ~/.config/codex:/home/node/.config/codex:ro
```
