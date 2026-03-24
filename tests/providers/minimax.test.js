/**
 * MiniMax Provider Tests
 *
 * Tests for the MiniMax provider: error classification, model selection,
 * CLI builder, and SDK availability detection.
 */

const assert = require('assert');
const path = require('path');
const { getProvider } = require('../../src/providers');
const { normalizeProviderName, VALID_PROVIDERS } = require('../../lib/provider-names');
const { CAPABILITIES } = require('../../src/providers/capabilities');
const { buildCommand } = require('../../src/providers/minimax/cli-builder');

describe('MiniMax provider', () => {
  let provider;

  before(() => {
    provider = getProvider('minimax');
  });

  describe('provider registration', () => {
    it('is registered in VALID_PROVIDERS', () => {
      assert.ok(VALID_PROVIDERS.includes('minimax'));
    });

    it('normalizes provider name', () => {
      assert.strictEqual(normalizeProviderName('minimax'), 'minimax');
    });

    it('is instantiated via getProvider', () => {
      assert.ok(provider);
      assert.strictEqual(provider.name, 'minimax');
      assert.strictEqual(provider.displayName, 'MiniMax');
    });

    it('has capabilities defined', () => {
      assert.ok(CAPABILITIES.minimax);
      assert.strictEqual(CAPABILITIES.minimax.dockerIsolation, true);
      assert.strictEqual(CAPABILITIES.minimax.worktreeIsolation, true);
      assert.strictEqual(CAPABILITIES.minimax.streamJson, true);
      assert.strictEqual(CAPABILITIES.minimax.thinkingMode, true);
      assert.strictEqual(CAPABILITIES.minimax.reasoningEffort, false);
    });
  });

  describe('model catalog', () => {
    it('includes M2.7 and M2.5 models', () => {
      const catalog = provider.getModelCatalog();
      assert.ok(catalog['MiniMax-M2.7']);
      assert.ok(catalog['MiniMax-M2.7-highspeed']);
      assert.ok(catalog['MiniMax-M2.5']);
      assert.ok(catalog['MiniMax-M2.5-highspeed']);
    });

    it('has correct model ranks', () => {
      const catalog = provider.getModelCatalog();
      assert.strictEqual(catalog['MiniMax-M2.7'].rank, 3);
      assert.strictEqual(catalog['MiniMax-M2.5-highspeed'].rank, 1);
    });
  });

  describe('level mapping', () => {
    it('maps level1 to M2.5-highspeed', () => {
      const spec = provider.resolveModelSpec('level1');
      assert.strictEqual(spec.model, 'MiniMax-M2.5-highspeed');
      assert.strictEqual(spec.level, 'level1');
    });

    it('maps level2 to M2.7 (default)', () => {
      const spec = provider.resolveModelSpec('level2');
      assert.strictEqual(spec.model, 'MiniMax-M2.7');
      assert.strictEqual(spec.level, 'level2');
    });

    it('maps level3 to M2.7', () => {
      const spec = provider.resolveModelSpec('level3');
      assert.strictEqual(spec.model, 'MiniMax-M2.7');
      assert.strictEqual(spec.level, 'level3');
    });

    it('default level is level2', () => {
      assert.strictEqual(provider.getDefaultLevel(), 'level2');
    });

    it('supports level overrides', () => {
      const spec = provider.resolveModelSpec('level1', {
        level1: { model: 'MiniMax-M2.7-highspeed' },
      });
      assert.strictEqual(spec.model, 'MiniMax-M2.7-highspeed');
    });
  });

  describe('error classification', () => {
    it('classifies rate limit errors as retryable', () => {
      const err = new Error('rate limit exceeded');
      assert.strictEqual(provider.isRetryableError(err), true);
    });

    it('classifies too many requests as retryable', () => {
      const err = new Error('too many requests');
      assert.strictEqual(provider.isRetryableError(err), true);
    });

    it('classifies server errors as retryable', () => {
      const err = new Error('server error');
      assert.strictEqual(provider.isRetryableError(err), true);
    });

    it('classifies invalid API key as permanent', () => {
      const err = new Error('invalid api key');
      assert.strictEqual(provider.isRetryableError(err), false);
    });

    it('classifies invalid_request as permanent', () => {
      const err = new Error('invalid_request: bad parameter');
      assert.strictEqual(provider.isRetryableError(err), false);
    });

    it('classifies HTTP 429 as retryable', () => {
      const err = { status: 429, message: 'Rate limited' };
      assert.strictEqual(provider.isRetryableError(err), true);
    });

    it('classifies HTTP 500 as retryable', () => {
      const err = { status: 500, message: 'Internal server error' };
      assert.strictEqual(provider.isRetryableError(err), true);
    });

    it('classifies HTTP 401 as permanent', () => {
      const err = { status: 401, message: 'Unauthorized' };
      assert.strictEqual(provider.isRetryableError(err), false);
    });
  });

  describe('SDK support', () => {
    it('returns MINIMAX_API_KEY as SDK env var', () => {
      assert.strictEqual(provider.getSDKEnvVar(), 'MINIMAX_API_KEY');
    });

    it('isSDKConfigured returns true when key is set', () => {
      const original = process.env.MINIMAX_API_KEY;
      process.env.MINIMAX_API_KEY = 'test-key';
      assert.strictEqual(provider.isSDKConfigured(), true);
      if (original) {
        process.env.MINIMAX_API_KEY = original;
      } else {
        delete process.env.MINIMAX_API_KEY;
      }
    });

    it('isSDKConfigured returns false when key is not set', () => {
      const original = process.env.MINIMAX_API_KEY;
      delete process.env.MINIMAX_API_KEY;
      assert.strictEqual(provider.isSDKConfigured(), false);
      if (original) process.env.MINIMAX_API_KEY = original;
    });
  });

  describe('CLI features', () => {
    it('reports supported features', () => {
      const features = provider.getCliFeatures();
      assert.strictEqual(features.supportsJson, true);
      assert.strictEqual(features.supportsModel, true);
      assert.strictEqual(features.supportsAutoApprove, false);
      assert.strictEqual(features.supportsVariant, false);
    });

    it('provides install instructions', () => {
      const instructions = provider.getInstallInstructions();
      assert.ok(instructions.includes('MINIMAX_API_KEY'));
    });

    it('provides auth instructions', () => {
      const instructions = provider.getAuthInstructions();
      assert.ok(instructions.includes('MINIMAX_API_KEY'));
    });
  });

  describe('settings', () => {
    it('includes minimaxApiKey in default settings', () => {
      const defaults = provider.getDefaultSettings();
      assert.ok('minimaxApiKey' in defaults);
      assert.strictEqual(defaults.minimaxApiKey, null);
    });

    it('includes minimaxApiKey in settings fields', () => {
      const fields = provider.getSettingsFields();
      assert.ok(fields.includes('minimaxApiKey'));
    });

    it('validates valid settings', () => {
      const error = provider.validateSettings({
        maxLevel: 'level3',
        minLevel: 'level1',
        defaultLevel: 'level2',
        levelOverrides: {},
      });
      assert.strictEqual(error, null);
    });

    it('validates minimaxApiKey type', () => {
      const error = provider.validateSettings({
        maxLevel: 'level3',
        minLevel: 'level1',
        defaultLevel: 'level2',
        levelOverrides: {},
        minimaxApiKey: 123,
      });
      assert.ok(error);
      assert.ok(error.includes('string or null'));
    });
  });
});

describe('MiniMax CLI Builder', () => {
  it('uses node as binary', () => {
    const result = buildCommand('test prompt', {});
    assert.strictEqual(result.binary, process.execPath);
  });

  it('includes cli-wrapper.js in args', () => {
    const result = buildCommand('test prompt', {});
    const wrapperArg = result.args.find((a) => a.includes('cli-wrapper.js'));
    assert.ok(wrapperArg, 'Should include cli-wrapper.js path');
  });

  it('passes model from modelSpec', () => {
    const result = buildCommand('test', {
      modelSpec: { model: 'MiniMax-M2.5-highspeed' },
    });
    assert.ok(result.args.includes('--model'));
    assert.ok(result.args.includes('MiniMax-M2.5-highspeed'));
  });

  it('passes --json flag for json output format', () => {
    const result = buildCommand('test', { outputFormat: 'json' });
    assert.ok(result.args.includes('--json'));
  });

  it('passes --json flag for stream-json output format', () => {
    const result = buildCommand('test', { outputFormat: 'stream-json' });
    assert.ok(result.args.includes('--json'));
  });

  it('injects JSON schema into context when provided', () => {
    const schema = { type: 'object', properties: { result: { type: 'string' } } };
    const result = buildCommand('test prompt', { jsonSchema: schema });

    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT (CRITICAL - REQUIRED)'));
    assert.ok(finalContext.includes('You MUST respond with a JSON object'));
    assert.ok(finalContext.includes('"result"'));
  });

  it('does NOT inject schema when no jsonSchema provided', () => {
    const result = buildCommand('test prompt', {});
    const finalContext = result.args[result.args.length - 1];
    assert.strictEqual(finalContext, 'test prompt');
    assert.ok(!finalContext.includes('OUTPUT FORMAT'));
  });

  it('passes MINIMAX_API_KEY from environment', () => {
    const original = process.env.MINIMAX_API_KEY;
    process.env.MINIMAX_API_KEY = 'test-key-123';
    const result = buildCommand('test', {});
    assert.strictEqual(result.env.MINIMAX_API_KEY, 'test-key-123');
    if (original) {
      process.env.MINIMAX_API_KEY = original;
    } else {
      delete process.env.MINIMAX_API_KEY;
    }
  });

  it('handles string jsonSchema', () => {
    const schemaStr = '{"type":"object","properties":{"bar":{"type":"number"}}}';
    const result = buildCommand('test context', { jsonSchema: schemaStr });
    const finalContext = result.args[result.args.length - 1];
    assert.ok(finalContext.includes('## OUTPUT FORMAT'));
    assert.ok(finalContext.includes('"bar"'));
  });
});
