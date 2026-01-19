# Claude Provider

## Authentication

### Interactive Login (Default)

```bash
claude login
```

### API Key (Direct Anthropic)

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```

Or persist in settings:

```bash
zeroshot settings set providerSettings.claude.anthropicApiKey sk-ant-...
```

### AWS Bedrock

Generate a long-term API key from [AWS Bedrock Console](https://console.aws.amazon.com/bedrock/home#/api-keys) → API keys → Generate long-term API key.

```bash
export CLAUDE_CODE_USE_BEDROCK=1
export AWS_REGION=eu-central-1
export AWS_BEARER_TOKEN_BEDROCK=ABSK...
```

Or persist in settings (recommended):

```bash
zeroshot settings set providerSettings.claude.bedrockApiKey "ABSK..."
zeroshot settings set providerSettings.claude.bedrockRegion eu-central-1
```

When using settings, `CLAUDE_CODE_USE_BEDROCK=1` is set automatically.
