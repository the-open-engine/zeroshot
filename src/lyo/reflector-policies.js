/**
 * Reflector policies (design doc §2). A reflector is ANY object:
 *
 *   {
 *     name: string,
 *     version: integer,
 *     reflect({ message, failure_class, cue }) -> { explanation, intervention }
 *   }
 *
 * `message` is the rejected VALIDATION_RESULT ledger message. `failure_class`
 * and `cue` come from the deterministic failure classifier — the grounding
 * anchor; a reflector may NOT move them (they key retrieval and attribution).
 * `explanation` is WHY the failure happened, abstracted; `intervention` is
 * WHAT TO DO DIFFERENTLY, transferable. The observer uses `intervention`
 * verbatim as the base guidance text sent to the implementation agent, so a
 * reflector controls both what is stored and what is delivered.
 *
 * Peirce: the reflector is the loop's ABDUCTION step (hypothesis generation).
 * Deduction (decision log) and induction (Beta-Bernoulli counters, Wilson
 * gate) live elsewhere and grade whatever the reflector proposes — so the
 * reflector may be speculative, but must never be trusted. See
 * docs/lyo-reflector-design (when written) for the elaborator contract:
 * free-text elaboration as intermediary, derived (never elicited) scores.
 *
 * Swapping the reflector is a config change, never an observer/store change:
 * cluster.config.lyo.reflector = '<name>@<version>' (registry id) or an
 * injected object via attachLyoObserver({ reflector }). The CREATE/EDIT delta
 * payload records the authoring reflector id, so outcome lift can be compared
 * per reflector (A/B: template@1 vs elaborator@1) through the existing
 * decision/outcome join.
 *
 * Containment (Appendix B.4): a reflector runs inside the observer's
 * try/catch; ANY failure — unknown id, throw, malformed return — falls back
 * to template@1, which is pure and never throws. Learning never blocks a run.
 */

const EXPLANATION_MAX_LENGTH = 500;

function truncate(text, maxLength) {
  const value = String(text || '');
  return value.length > maxLength ? `${value.slice(0, maxLength - 3)}...` : value;
}

function formatValidationFeedback(message) {
  const parts = [];
  const text = message.content?.text;
  const errors = message.content?.data?.errors;

  if (text) {
    parts.push(text);
  }

  if (Array.isArray(errors) && errors.length > 0) {
    parts.push(`Errors:\n${errors.map((error) => `- ${error}`).join('\n')}`);
  }

  return parts.join('\n\n') || 'Validator rejected the last result without details.';
}

function buildGuidanceText(message) {
  return `Address the validator feedback before retrying.\n\nLatest validation:\n${formatValidationFeedback(message)}`;
}

// template@1 — the v0 reflector: raw validator feedback, truncated, wrapped
// in a fixed instruction. No abstraction; it exists so the loop runs and so
// future reflectors have a baseline to beat (and a safe fallback).
const TEMPLATE_REFLECTOR = {
  name: 'template',
  version: 1,
  reflect({ message }) {
    return {
      explanation: truncate(formatValidationFeedback(message), EXPLANATION_MAX_LENGTH),
      intervention: buildGuidanceText(message),
    };
  },
};

const DEFAULT_REFLECTOR = TEMPLATE_REFLECTOR;

function reflectorId(reflector) {
  return `${reflector.name}@${reflector.version}`;
}

// A reflection is only admissible if both fields are strings — anything else
// (missing fields, numbers, null) is a reflector bug and falls back.
function isValidReflection(reflection) {
  return (
    !!reflection &&
    typeof reflection.explanation === 'string' &&
    typeof reflection.intervention === 'string'
  );
}

// String-addressable reflectors ('name@version'); object reflectors can
// always be injected directly without registration.
const REFLECTOR_REGISTRY = new Map([[reflectorId(TEMPLATE_REFLECTOR), TEMPLATE_REFLECTOR]]);

// Accepts a reflector object, a registry id string, or null (default).
function resolveReflector(ref) {
  if (!ref) {
    return DEFAULT_REFLECTOR;
  }
  if (typeof ref.reflect === 'function') {
    return ref;
  }
  const reflector = REFLECTOR_REGISTRY.get(String(ref));
  if (!reflector) {
    throw new Error(`unknown reflector: ${ref}`);
  }
  return reflector;
}

module.exports = {
  TEMPLATE_REFLECTOR,
  DEFAULT_REFLECTOR,
  REFLECTOR_REGISTRY,
  EXPLANATION_MAX_LENGTH,
  reflectorId,
  isValidReflection,
  resolveReflector,
  formatValidationFeedback,
  buildGuidanceText,
};
