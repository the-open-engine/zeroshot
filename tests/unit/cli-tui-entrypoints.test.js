/**
 * Test: CLI TUI Entrypoints
 *
 * Verifies provider-specific entrypoints map to TUI provider override.
 */

const assert = require('assert');
const { resolveTuiProviderOverride } = require('../../lib/tui-launcher');

function buildEntrypointOptions(providerName) {
  return { providerOverride: resolveTuiProviderOverride({ provider: providerName }) };
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
