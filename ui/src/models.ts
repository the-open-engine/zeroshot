// Optional UI suggestions, verified 2026-09-14. Never used for admission, inference,
// or availability checks. Custom values and existing selections remain unchanged.
// Sources: developers.openai.com/api/docs/models/gpt-5.6-sol,
// platform.claude.com/docs/en/models/overview, openrouter.ai/openai/gpt-5.6-sol,
// openrouter.ai/anthropic/claude-sonnet-5,
// docs.aws.amazon.com/bedrock/latest/userguide/model-card-openai-gpt-56-sol.html,
// docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-sonnet-5.html.
// Astra/Fable additions: developers.openai.com/api/docs/models/gpt-6-astra,
// platform.claude.com/docs/en/models/fable-5-1/overview,
// openrouter.ai/openai/gpt-6-astra, openrouter.ai/anthropic/claude-fable-5.1,
// docs.aws.amazon.com/bedrock/latest/userguide/model-card-openai-gpt-6-astra.html,
// docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-fable-5-1.html.
const suggestions: Record<string, Record<string, string[]>> = {
  codex: {
    openai: ['gpt-6-astra', 'gpt-5.6-sol', 'gpt-5.6-terra'],
    openrouter: ['openai/gpt-6-astra', 'openai/gpt-5.6-sol', 'openai/gpt-5.6-terra'],
    // Codex's built-in Bedrock provider uses the mantle endpoint (AWS web-search docs).
    bedrock: ['openai.gpt-6-astra', 'openai.gpt-5.6-sol', 'openai.gpt-5.6-terra'],
  },
  claude: {
    anthropic: ['claude-fable-5-1', 'claude-sonnet-5', 'claude-opus-5'],
    openrouter: [
      'anthropic/claude-fable-5.1',
      'anthropic/claude-sonnet-5',
      'anthropic/claude-opus-5',
    ],
    bedrock: [
      'global.anthropic.claude-fable-5-1',
      'global.anthropic.claude-sonnet-5',
      'global.anthropic.claude-opus-5',
    ],
  },
};
export function suggestedModels(harness: string, provider: string): string[] {
  const entry = Object.hasOwn(suggestions, harness) ? suggestions[harness] : undefined;
  return entry && Object.hasOwn(entry, provider) ? entry[provider] : [];
}
