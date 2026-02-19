// Codex defaults to gpt-5.3-codex; levels vary by reasoning effort only.
const MODEL_CATALOG = {
  'gpt-5.3-codex': { rank: 2 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: 'gpt-5.3-codex', reasoningEffort: 'medium' },
  level2: { rank: 2, model: 'gpt-5.3-codex', reasoningEffort: 'high' },
  level3: { rank: 3, model: 'gpt-5.3-codex', reasoningEffort: 'xhigh' },
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
