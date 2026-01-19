const MODEL_CATALOG = {
  'opencode/big-pickle': { rank: 1 },
  'opencode/glm-4.7-free': { rank: 1 },
  'opencode/gpt-5-nano': { rank: 1 },
  'opencode/grok-code': { rank: 1 },
  'opencode/minimax-m2.1-free': { rank: 1 },
  'google/gemini-1.5-flash': { rank: 1 },
  'google/gemini-1.5-flash-8b': { rank: 1 },
  'google/gemini-1.5-pro': { rank: 1 },
  'google/gemini-2.0-flash': { rank: 1 },
  'google/gemini-2.0-flash-lite': { rank: 1 },
  'google/gemini-2.5-flash': { rank: 1 },
  'google/gemini-2.5-flash-image': { rank: 1 },
  'google/gemini-2.5-flash-image-preview': { rank: 1 },
  'google/gemini-2.5-flash-lite': { rank: 1 },
  'google/gemini-2.5-flash-lite-preview-06-17': { rank: 1 },
  'google/gemini-2.5-flash-lite-preview-09-2025': { rank: 1 },
  'google/gemini-2.5-flash-preview-04-17': { rank: 1 },
  'google/gemini-2.5-flash-preview-05-20': { rank: 1 },
  'google/gemini-2.5-flash-preview-09-2025': { rank: 1 },
  'google/gemini-2.5-flash-preview-tts': { rank: 1 },
  'google/gemini-2.5-pro': { rank: 1 },
  'google/gemini-2.5-pro-preview-05-06': { rank: 1 },
  'google/gemini-2.5-pro-preview-06-05': { rank: 1 },
  'google/gemini-2.5-pro-preview-tts': { rank: 1 },
  'google/gemini-3-flash-preview': { rank: 1 },
  'google/gemini-3-pro-preview': { rank: 1 },
  'google/gemini-embedding-001': { rank: 1 },
  'google/gemini-flash-latest': { rank: 1 },
  'google/gemini-flash-lite-latest': { rank: 1 },
  'google/gemini-live-2.5-flash': { rank: 1 },
  'google/gemini-live-2.5-flash-preview-native-audio': { rank: 1 },
  'openai/gpt-5.1-codex-max': { rank: 1 },
  'openai/gpt-5.1-codex-mini': { rank: 1 },
  'openai/gpt-5.2': { rank: 1 },
  'openai/gpt-5.2-codex': { rank: 1 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: null, reasoningEffort: 'low' },
  level2: { rank: 2, model: null, reasoningEffort: 'medium' },
  level3: { rank: 3, model: null, reasoningEffort: 'high' },
};

const DEFAULT_LEVEL = 'level2';
const DEFAULT_MAX_LEVEL = 'level3';
const DEFAULT_MIN_LEVEL = 'level1';

module.exports = {
  MODEL_CATALOG,
  LEVEL_MAPPING,
  DEFAULT_LEVEL,
  DEFAULT_MAX_LEVEL,
  DEFAULT_MIN_LEVEL,
};
