const assert = require('assert');

const {
  INVERSION_MAP,
  executorFamily,
  invertedReflectorModel,
} = require('../../src/lyo/model-inversion');
const { resolveReflector } = require('../../src/lyo/reflector-policies');

describe('model inversion routing', function () {
  it('parses the executor family from provider-prefixed ids', function () {
    assert.strictEqual(executorFamily('claude:level2'), 'claude');
    assert.strictEqual(executorFamily('OpenAI:gpt-5'), 'openai');
    assert.strictEqual(executorFamily('google:gemini-2.5-pro'), 'google');
    assert.strictEqual(executorFamily('level2'), null); // no provider prefix
    assert.strictEqual(executorFamily(':orphan'), null);
    assert.strictEqual(executorFamily(null), null);
    assert.strictEqual(executorFamily(undefined), null);
  });

  it('routes each known executor family to a different-family reflector', function () {
    assert.strictEqual(invertedReflectorModel('claude:level2'), 'openai/gpt-4o-mini');
    assert.strictEqual(invertedReflectorModel('anthropic:opus'), 'openai/gpt-4o-mini');
    assert.strictEqual(invertedReflectorModel('openai:gpt-5'), 'anthropic/claude-3.5-haiku');
    assert.strictEqual(invertedReflectorModel('codex:level2'), 'anthropic/claude-3.5-haiku');
    assert.strictEqual(invertedReflectorModel('google:gemini-2.5-pro'), 'openai/gpt-4o-mini');
  });

  it('never routes to the same family it detected', function () {
    for (const [family, reflectorModel] of INVERSION_MAP) {
      assert.ok(
        !reflectorModel.toLowerCase().startsWith(`${family}/`),
        `${family} must not invert to ${reflectorModel}`
      );
    }
  });

  it('returns null for unknown families so callers fall through to env/default', function () {
    assert.strictEqual(invertedReflectorModel('opencode:level2'), null);
    assert.strictEqual(invertedReflectorModel(null), null);
    assert.strictEqual(invertedReflectorModel('level2'), null);
  });

  it('composes with the reflector registry: inverted ctx reaches the elaborator factory', function () {
    // The observer path: ctx.model = configured ?? invertedReflectorModel(executor).
    const crossFamily = resolveReflector('elaborator@1', {
      model: invertedReflectorModel('claude:level2'),
    });
    assert.strictEqual(crossFamily.model, 'openai/gpt-4o-mini');
    const sameFamilyControl = resolveReflector('elaborator@1', {
      model: invertedReflectorModel('openai:gpt-5'),
    });
    assert.strictEqual(sameFamilyControl.model, 'anthropic/claude-3.5-haiku');
    // Explicit config wins: the factory takes the ctx model as-is.
    const explicit = resolveReflector('elaborator@1', { model: 'google/gemini-2.0-flash' });
    assert.strictEqual(explicit.model, 'google/gemini-2.0-flash');
  });
});
