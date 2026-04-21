const MODEL_CATALOG = {
  'gpt-5-mini': { rank: 1 },
  'gpt-5': { rank: 2 },
  'claude-sonnet-4.5': { rank: 2 },
  'claude-opus-4.6': { rank: 3 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: 'gpt-5-mini', reasoningEffort: null },
  level2: { rank: 2, model: 'claude-sonnet-4.5', reasoningEffort: null },
  level3: { rank: 3, model: 'claude-opus-4.6', reasoningEffort: null },
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
