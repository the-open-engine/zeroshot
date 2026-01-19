// Gemini CLI model names
// Model is optional - Gemini defaults to best available
const MODEL_CATALOG = {
  'gemini-2.5-pro': { rank: 3 },
  'gemini-2.0-flash': { rank: 1 },
};

const LEVEL_MAPPING = {
  level1: { rank: 1, model: null },
  level2: { rank: 2, model: null },
  level3: { rank: 3, model: null },
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
