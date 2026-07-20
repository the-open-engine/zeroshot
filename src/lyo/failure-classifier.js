/**
 * failure-classifier - deterministic keyword classifier for validation failures.
 *
 * v0.1 deliberately uses keyword matching, NOT an LLM: explanation-extraction
 * prompts are an explicit non-goal (design doc §8). The taxonomy is seeded from
 * TRAIL (design doc §3): goal_deviation, tool_selection, context_handling,
 * orchestration, output_generation, system_execution.
 */

const FAILURE_CLASSES = [
  'goal_deviation',
  'tool_selection',
  'context_handling',
  'orchestration',
  'output_generation',
  'system_execution',
];

const DEFAULT_FAILURE_CLASS = 'output_generation';

const CUE_MAX_LENGTH = 120;
const FALLBACK_CUE = 'validation rejected (no details)';

// Keyword rules. Rule order is the tie-breaker when two classes match the same
// number of distinct keywords; otherwise the class with the most distinct
// keyword hits wins. Patterns are matched case-insensitively against
// content.text + data.errors[] + stringified data.criteriaResults.
const RULES = [
  {
    failure_class: 'goal_deviation',
    patterns: [/\bmisunderstand/, /\brequirement/, /\bacceptance[\s-]criteria/, /\bgoal/],
  },
  {
    failure_class: 'orchestration',
    patterns: [
      /\btime[\s-]?out/,
      /\btimed out/,
      /\bstuck/,
      /\bloop/,
      /\bhandoff/,
      /\bdeadlock/,
      /\bagent/,
    ],
  },
  {
    failure_class: 'system_execution',
    patterns: [
      /\bgit/,
      /\bpush/,
      /\bmerge/,
      /\bbranch/,
      /\bpr\b/,
      /\bcommit/,
      /\bpermission/,
      /\bfile[\s-]not[\s-]found/,
      /\benoent/,
      /\bcommand/,
    ],
  },
  {
    failure_class: 'tool_selection',
    patterns: [/\bwrong[\s-]tool/, /\bapi[\s-]misuse/, /\bparameter/],
  },
  {
    failure_class: 'context_handling',
    patterns: [
      /\bcontext/,
      /\btoken/,
      /\btruncat/,
      /\bmissing[\s-]info(?:rmation)?/,
      /\bambiguous/,
    ],
  },
  {
    failure_class: 'output_generation',
    patterns: [
      /\btest/,
      /\bcoverage/,
      /\blint/,
      /\btype[\s-]?check/,
      /\bbuild/,
      /\bspec/,
      /\bformat/,
    ],
  },
];

/**
 * Normalize a cue for storage and matching: lowercase, collapse whitespace, trim.
 * The lesson store treats the normalized form as the merge key for trigger_cue.
 */
function normalizeCue(text) {
  return String(text || '')
    .toLowerCase()
    .replace(/\s+/g, ' ')
    .trim();
}

function safeStringify(value) {
  if (value === undefined || value === null) return '';
  try {
    return JSON.stringify(value);
  } catch {
    return '';
  }
}

/**
 * The cue becomes the lesson's trigger_cue: normalized first error line (or
 * first text line), truncated to ~120 chars.
 */
function extractCue(validationMessage) {
  const content = validationMessage?.content || {};
  const errors = content.data?.errors;
  let raw = '';

  if (Array.isArray(errors)) {
    const firstError = errors.find((error) => typeof error === 'string' && error.trim());
    if (firstError) {
      raw = firstError.split('\n')[0];
    }
  }

  if (!raw && typeof content.text === 'string') {
    raw = content.text.split('\n')[0];
  }

  return normalizeCue(raw).slice(0, CUE_MAX_LENGTH).trim() || FALLBACK_CUE;
}

/**
 * Classify a VALIDATION_RESULT message into a failure class and a short cue.
 * @param {Object} validationMessage - Message with content.text / content.data
 * @returns {{ failure_class: string, cue: string }}
 */
function classifyValidationFailure(validationMessage) {
  const content = validationMessage?.content || {};
  const data = content.data || {};
  const parts = [];

  if (typeof content.text === 'string') {
    parts.push(content.text);
  }
  if (Array.isArray(data.errors)) {
    parts.push(...data.errors.map(String));
  }
  const criteriaResults = safeStringify(data.criteriaResults);
  if (criteriaResults) {
    parts.push(criteriaResults);
  }

  const haystack = parts.join('\n').toLowerCase();

  let bestClass = null;
  let bestScore = 0;
  for (const rule of RULES) {
    const score = rule.patterns.reduce(
      (hits, pattern) => (pattern.test(haystack) ? hits + 1 : hits),
      0
    );
    if (score > bestScore) {
      bestScore = score;
      bestClass = rule.failure_class;
    }
  }

  return {
    failure_class: bestClass || DEFAULT_FAILURE_CLASS,
    cue: extractCue(validationMessage),
  };
}

module.exports = {
  FAILURE_CLASSES,
  classifyValidationFailure,
  normalizeCue,
};
