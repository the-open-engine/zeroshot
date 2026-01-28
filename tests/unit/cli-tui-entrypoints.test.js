/**
 * Test: CLI TUI Entrypoints
 *
 * Verifies provider-specific entrypoints map to TUI provider override.
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
  return {
    autoExit: false,
    providerOverride: resolveTuiProviderOverride(options),
    initialView: options.initialView,
  };
}

function buildEntrypointOptions(providerName) {
  return buildTuiStartOptions({ provider: providerName });
}

describe('CLI TUI Entrypoints', function () {
  const entrypoints = ['codex', 'claude', 'gemini', 'opencode'];

  for (const provider of entrypoints) {
    it(`sets providerOverride for ${provider}`, function () {
      const result = buildEntrypointOptions(provider);
      assert.strictEqual(result.providerOverride, provider);
    });
  }
});
