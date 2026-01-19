// Codex CLI - use null to let CLI pick its default model
// Levels vary by reasoning effort only
const MODEL_CATALOG = {};

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
