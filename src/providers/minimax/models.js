const MODEL_CATALOG = {
  'MiniMax-M2.7': { rank: 3, contextWindow: 1000000 },
  'MiniMax-M2.7-highspeed': { rank: 2, contextWindow: 1000000 },
  'MiniMax-M2.5': { rank: 2, contextWindow: 204000 },
  'MiniMax-M2.5-highspeed': { rank: 1, contextWindow: 204000 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: 'MiniMax-M2.5-highspeed', reasoningEffort: null },
  level2: { rank: 2, model: 'MiniMax-M2.7', reasoningEffort: null },
  level3: { rank: 3, model: 'MiniMax-M2.7', reasoningEffort: null },
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
