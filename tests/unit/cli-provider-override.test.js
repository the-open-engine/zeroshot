/**
 * Test: CLI Provider Override
 *
 * Verifies that provider override is only applied when explicitly set
 * via --provider or ZEROSHOT_PROVIDER.
 */

const assert = require('assert');

function normalizeProviderName(name) {
  if (!name || typeof name !== 'string') return name;
  const normalized = name.toLowerCase();
  if (normalized === 'anthropic') return 'claude';
  if (normalized === 'openai') return 'codex';
  if (normalized === 'google') return 'gemini';
  return normalized;
}

// Mirrors resolveProviderOverride in cli/index.js
function resolveProviderOverride(options) {
  const override = options.provider || process.env.ZEROSHOT_PROVIDER;
  if (!override || (typeof override === 'string' && !override.trim())) {
    return null;
  }
  return normalizeProviderName(override);
}

describe('CLI Provider Override', function () {
  const originalEnv = process.env.ZEROSHOT_PROVIDER;

  afterEach(function () {
    if (originalEnv === undefined) {
      delete process.env.ZEROSHOT_PROVIDER;
    } else {
      process.env.ZEROSHOT_PROVIDER = originalEnv;
    }
  });

  it('returns null when no override is set', function () {
    delete process.env.ZEROSHOT_PROVIDER;
    const result = resolveProviderOverride({});
    assert.strictEqual(result, null);
  });

  it('uses --provider when provided', function () {
    delete process.env.ZEROSHOT_PROVIDER;
    const result = resolveProviderOverride({ provider: 'claude' });
    assert.strictEqual(result, 'claude');
  });

  it('normalizes provider aliases', function () {
    delete process.env.ZEROSHOT_PROVIDER;
    const result = resolveProviderOverride({ provider: 'Anthropic' });
    assert.strictEqual(result, 'claude');
  });

  it('uses ZEROSHOT_PROVIDER when --provider is missing', function () {
    process.env.ZEROSHOT_PROVIDER = 'codex';
    const result = resolveProviderOverride({});
    assert.strictEqual(result, 'codex');
  });

  it('ignores empty ZEROSHOT_PROVIDER', function () {
    process.env.ZEROSHOT_PROVIDER = '   ';
    const result = resolveProviderOverride({});
    assert.strictEqual(result, null);
  });

  it('prefers --provider over ZEROSHOT_PROVIDER', function () {
    process.env.ZEROSHOT_PROVIDER = 'gemini';
    const result = resolveProviderOverride({ provider: 'claude' });
    assert.strictEqual(result, 'claude');
  });
});
