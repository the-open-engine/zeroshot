/**
 * Schema utilities for normalizing LLM output before validation.
 *
 * PROBLEM: LLMs (Claude, Gemini, Codex) via any interface (CLI, API) may return
 * enum values that don't exactly match the schema (e.g., "simple" vs "SIMPLE").
 *
 * SOLUTION: Normalize enum values BEFORE validation. Provider-agnostic.
 */

const ENUM_VARIATIONS = {
  // taskType variations
  BUG: 'DEBUG',
  FIX: 'DEBUG',
  BUGFIX: 'DEBUG',
  BUG_FIX: 'DEBUG',
  INVESTIGATE: 'DEBUG',
  TROUBLESHOOT: 'DEBUG',
  IMPLEMENT: 'TASK',
  BUILD: 'TASK',
  CREATE: 'TASK',
  ADD: 'TASK',
  FEATURE: 'TASK',
  QUESTION: 'INQUIRY',
  ASK: 'INQUIRY',
  EXPLORE: 'INQUIRY',
  RESEARCH: 'INQUIRY',
  UNDERSTAND: 'INQUIRY',
  // complexity variations
  EASY: 'TRIVIAL',
  BASIC: 'SIMPLE',
  MINOR: 'SIMPLE',
  MODERATE: 'STANDARD',
  MEDIUM: 'STANDARD',
  NORMAL: 'STANDARD',
  HARD: 'STANDARD',
  COMPLEX: 'CRITICAL',
  RISKY: 'CRITICAL',
  HIGH_RISK: 'CRITICAL',
  DANGEROUS: 'CRITICAL',
};

/**
 * Normalize enum values in parsed JSON to match schema definitions.
 *
 * Handles:
 * - Case mismatches: "simple" → "SIMPLE"
 * - Whitespace: " SIMPLE " → "SIMPLE"
 * - Common variations: "bug" → "DEBUG", "fix" → "DEBUG"
 *
 * @param {Object} result - Parsed JSON result from LLM
 * @param {Object} schema - JSON schema with enum definitions
 * @returns {Object} Normalized result (mutates and returns same object)
 */
function normalizeEnumValues(result, schema) {
  if (!result || typeof result !== 'object' || !schema?.properties) {
    return result;
  }

  for (const [key, propSchema] of Object.entries(schema.properties)) {
    const matched = normalizeEnumValue({
      result,
      key,
      propSchema,
    });
    if (matched) {
      continue;
    }

    // Recursively handle nested objects
    normalizeNestedValues(result, propSchema, key);
  }

  return result;
}

function normalizeEnumValue({ result, key, propSchema }) {
  if (!propSchema.enum || typeof result[key] !== 'string') {
    return false;
  }

  const rawValue = result[key];
  let value = rawValue.trim().toUpperCase();
  value = normalizeEnumCopyValue(value, propSchema.enum, key, rawValue);

  // Find exact match (case-insensitive)
  const match = findEnumMatch(propSchema.enum, value);
  if (match) {
    result[key] = match;
    return true;
  }

  // Common variations mapping
  const variation = ENUM_VARIATIONS[value];
  if (variation && propSchema.enum.includes(variation)) {
    result[key] = variation;
  }

  return false;
}

function normalizeEnumCopyValue(value, enumValues, key, rawValue) {
  // DETECT: Model copied the enum list instead of choosing (e.g., "TRIVIAL|SIMPLE|STANDARD")
  if (!value.includes('|')) {
    return value;
  }

  const parts = value.split('|').map((p) => p.trim());
  // Check if this looks like the enum list was copied verbatim
  const matchCount = parts.filter((p) => enumValues.includes(p)).length;
  if (matchCount < 2) {
    return value;
  }

  // Model copied the format - pick the first valid option and warn
  const firstValid = parts.find((p) => enumValues.includes(p));
  if (!firstValid) {
    return value;
  }

  console.warn(
    `⚠️  Model copied enum format instead of choosing. Field "${key}" had "${rawValue}", using "${firstValid}"`
  );
  return firstValid;
}

function findEnumMatch(enumValues, value) {
  return enumValues.find((entry) => entry.toUpperCase() === value);
}

function normalizeNestedValues(result, propSchema, key) {
  // Recursively handle nested objects
  if (propSchema.type === 'object' && propSchema.properties && result[key]) {
    normalizeEnumValues(result[key], propSchema);
  }

  // Handle arrays of objects
  if (propSchema.type === 'array' && propSchema.items?.properties && Array.isArray(result[key])) {
    for (const item of result[key]) {
      normalizeEnumValues(item, propSchema.items);
    }
  }
}

module.exports = {
  normalizeEnumValues,
};
