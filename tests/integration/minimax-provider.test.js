/**
 * MiniMax Provider Integration Tests
 *
 * Tests MiniMax provider integration with the provider system:
 * - Provider detection and listing
 * - Settings integration
 * - CLI wrapper execution (mock)
 */

const assert = require('assert');
const { getProvider, listProviders, detectProviders } = require('../../src/providers');
const { normalizeProviderName, normalizeProviderSettings } = require('../../lib/provider-names');

describe('MiniMax provider integration', () => {
  describe('provider system integration', () => {
    it('is listed among available providers', () => {
      const providers = listProviders();
      assert.ok(providers.includes('minimax'));
    });

    it('is included in detectProviders results', async () => {
      const detected = await detectProviders();
      assert.ok('minimax' in detected);
      assert.strictEqual(typeof detected.minimax.available, 'boolean');
    });

    it('can be instantiated and used like other providers', () => {
      const minimax = getProvider('minimax');
      const claude = getProvider('claude');

      // Both providers implement the same interface
      assert.strictEqual(typeof minimax.getModelCatalog, 'function');
      assert.strictEqual(typeof minimax.getLevelMapping, 'function');
      assert.strictEqual(typeof minimax.resolveModelSpec, 'function');
      assert.strictEqual(typeof minimax.buildCommand, 'function');
      assert.strictEqual(typeof minimax.parseEvent, 'function');
      assert.strictEqual(typeof minimax.isRetryableError, 'function');
      assert.strictEqual(typeof minimax.getDefaultSettings, 'function');
      assert.strictEqual(typeof minimax.validateSettings, 'function');

      // Same interface as Claude
      assert.strictEqual(typeof claude.getModelCatalog, 'function');
    });
  });

  describe('provider name normalization', () => {
    it('normalizes minimax to minimax', () => {
      assert.strictEqual(normalizeProviderName('minimax'), 'minimax');
    });

    it('normalizes case-insensitively via alias lookup', () => {
      // normalizeProviderName lowercases before alias lookup
      assert.strictEqual(normalizeProviderName('MiniMax'), 'minimax');
      assert.strictEqual(normalizeProviderName('MINIMAX'), 'minimax');
    });
  });

  describe('settings normalization', () => {
    it('normalizes minimax provider settings', () => {
      const settings = normalizeProviderSettings({
        minimax: {
          defaultLevel: 'level2',
          minimaxApiKey: null,
        },
      });

      assert.ok(settings.minimax);
      assert.strictEqual(settings.minimax.defaultLevel, 'level2');
    });
  });

  describe('model resolution workflow', () => {
    it('resolves full model spec for each level', () => {
      const provider = getProvider('minimax');

      const level1 = provider.resolveModelSpec('level1');
      assert.strictEqual(level1.model, 'MiniMax-M2.5-highspeed');
      assert.strictEqual(level1.level, 'level1');

      const level2 = provider.resolveModelSpec('level2');
      assert.strictEqual(level2.model, 'MiniMax-M2.7');
      assert.strictEqual(level2.level, 'level2');

      const level3 = provider.resolveModelSpec('level3');
      assert.strictEqual(level3.model, 'MiniMax-M2.7');
      assert.strictEqual(level3.level, 'level3');
    });

    it('validates level bounds correctly', () => {
      const provider = getProvider('minimax');

      assert.doesNotThrow(() => {
        provider.validateLevel('level2', 'level1', 'level3');
      });

      assert.throws(() => {
        provider.validateLevel('level4', 'level1', 'level3');
      }, /Invalid level/);
    });

    it('validates model IDs', () => {
      const provider = getProvider('minimax');

      assert.doesNotThrow(() => {
        provider.validateModelId('MiniMax-M2.7');
      });

      assert.throws(() => {
        provider.validateModelId('nonexistent-model');
      }, /Invalid model/);
    });
  });

  describe('CLI wrapper command generation', () => {
    it('generates a valid command with all options', () => {
      const provider = getProvider('minimax');
      const modelSpec = provider.resolveModelSpec('level2');

      const cmd = provider.buildCommand('Implement the feature described in the issue', {
        modelSpec,
        outputFormat: 'stream-json',
        jsonSchema: { type: 'object', properties: { result: { type: 'string' } } },
      });

      assert.strictEqual(cmd.binary, process.execPath);
      assert.ok(cmd.args.length > 0);
      assert.ok(cmd.args.some((a) => a.includes('cli-wrapper.js')));
      assert.ok(cmd.args.includes('--model'));
      assert.ok(cmd.args.includes('MiniMax-M2.7'));
      assert.ok(cmd.args.includes('--json'));

      // Schema should be injected into context (last arg)
      const context = cmd.args[cmd.args.length - 1];
      assert.ok(context.includes('OUTPUT FORMAT'));
    });
  });
});
