const MODEL_CATALOG = {
  haiku: { rank: 1 },
  sonnet: { rank: 2 },
  opus: { rank: 3 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: 'haiku' },
  level2: { rank: 2, model: 'sonnet' },
  level3: { rank: 3, model: 'opus' },
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
