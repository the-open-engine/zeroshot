/**
 * Model inversion routing (Greptile-style): the reflector should come from a
 * DIFFERENT model family than the executor whose failures it explains, so the
 * abducer doesn't share the executor's correlated blind spots (Greptile,
 * "Model Inversion", 2026 — cross-model review wins both directions, and
 * models miss the same bug categories they produce).
 *
 * This module decides ONLY the default. Precedence (observer):
 *   cluster.config.lyo.reflectorModel (explicit) > auto-invert (this) >
 *   OPENROUTER_LYO_MODEL env > elaborator's built-in default.
 * The resolved model is recorded on every lesson (reflector_model, v4), so
 * same-family vs cross-family stays measurable in v_lyo_pair_stats regardless
 * of how the choice was made.
 */

// Executor family -> cross-family reflector default (OpenRouter ids). The
// family is parsed from the observer's executor_model ('provider:model'),
// so keys are PROVIDER names, with common aliases.
const INVERSION_MAP = new Map([
  ['claude', 'openai/gpt-4o-mini'],
  ['anthropic', 'openai/gpt-4o-mini'],
  ['openai', 'anthropic/claude-3.5-haiku'],
  ['codex', 'anthropic/claude-3.5-haiku'],
  ['gpt', 'anthropic/claude-3.5-haiku'],
  ['google', 'openai/gpt-4o-mini'],
  ['gemini', 'openai/gpt-4o-mini'],
]);

// 'claude:level2' -> 'claude'; 'level2' (no prefix) -> null (family unknown).
function executorFamily(executorModel) {
  if (typeof executorModel !== 'string') {
    return null;
  }
  const separator = executorModel.indexOf(':');
  if (separator <= 0) {
    return null;
  }
  return executorModel.slice(0, separator).trim().toLowerCase() || null;
}

// The cross-family reflector default for this executor, or null when the
// family is unknown (caller then falls through to env/built-in defaults).
function invertedReflectorModel(executorModel) {
  const family = executorFamily(executorModel);
  if (!family) {
    return null;
  }
  return INVERSION_MAP.get(family) ?? null;
}

module.exports = { INVERSION_MAP, executorFamily, invertedReflectorModel };
