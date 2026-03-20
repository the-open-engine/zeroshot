const MODEL_CATALOG = {
  'deepseek/deepseek-v3.2': { rank: 2 },
  'zai-org/glm-5': { rank: 3 },
  'minimax/minimax-m2.5': { rank: 3 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: 'deepseek/deepseek-v3.2' },
  level2: { rank: 2, model: 'deepseek/deepseek-v3.2' },
  level3: { rank: 3, model: 'zai-org/glm-5' },
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
