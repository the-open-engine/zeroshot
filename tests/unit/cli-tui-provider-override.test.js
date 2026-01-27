/**
 * Test: CLI TUI Provider Override
 *
 * Verifies that --provider parsing for `zeroshot tui` normalizes and validates
 * the provider override passed to the TUI bootstrap.
 */

const assert = require('assert');
const { normalizeProviderName, VALID_PROVIDERS } = require('../../lib/provider-names');

// Mirrors resolveTuiProviderOverride in cli/index.js
function resolveTuiProviderOverride(options = {}) {
  const override = options.provider;
  if (!override || (typeof override === 'string' && !override.trim())) {
    return null;
  }
  const normalized = normalizeProviderName(override);
  if (!VALID_PROVIDERS.includes(normalized)) {
    throw new Error(`Unknown provider: ${normalized}. Valid: ${VALID_PROVIDERS.join(', ')}`);
  }
  return normalized;
}

// Mirrors the TUI bootstrap call options in cli/index.js
function buildTuiStartOptions(options = {}) {
  return { autoExit: false, providerOverride: resolveTuiProviderOverride(options) };
}

describe('CLI TUI Provider Override', function () {
  it('returns null when no override is set', function () {
    const result = resolveTuiProviderOverride({});
    assert.strictEqual(result, null);
  });

  it('passes provider override into TUI start options', function () {
    const result = buildTuiStartOptions({ provider: 'codex' });
    assert.strictEqual(result.providerOverride, 'codex');
  });

  it('normalizes provider aliases', function () {
    const result = resolveTuiProviderOverride({ provider: 'OpenAI' });
    assert.strictEqual(result, 'codex');
  });

  it('throws on unknown provider', function () {
    assert.throws(() => resolveTuiProviderOverride({ provider: 'invalid' }), /Unknown provider:/);
  });
});
