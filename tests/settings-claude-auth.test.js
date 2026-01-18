/**
 * Test: Claude authentication settings validation
 *
 * Tests that Claude-specific authentication settings are properly validated
 * through provider delegation
 */

const assert = require('assert');
const { validateSetting } = require('../lib/settings');
const { getProvider } = require('../src/providers');

describe('Claude authentication settings', function () {
  it('validates anthropicApiKey field through provider', function () {
    const provider = getProvider('claude');

    // Valid: null
    const error1 = provider.validateSettings({ anthropicApiKey: null });
    assert.strictEqual(error1, null);

    // Valid: sk-ant- prefix
    const error2 = provider.validateSettings({ anthropicApiKey: 'sk-ant-test123' });
    assert.strictEqual(error2, null);

    // Invalid: wrong prefix
    const error3 = provider.validateSettings({ anthropicApiKey: 'invalid-key' });
    assert.ok(error3);
    assert.ok(error3.includes('sk-ant-'));

    // Invalid: not a string
    const error4 = provider.validateSettings({ anthropicApiKey: 123 });
    assert.ok(error4);
    assert.ok(error4.includes('must be a string or null'));
  });

  it('validates bedrockApiKey field through provider', function () {
    const provider = getProvider('claude');

    // Valid: null
    const error1 = provider.validateSettings({ bedrockApiKey: null });
    assert.strictEqual(error1, null);

    // Valid: string
    const error2 = provider.validateSettings({ bedrockApiKey: 'some-token' });
    assert.strictEqual(error2, null);

    // Invalid: not a string
    const error3 = provider.validateSettings({ bedrockApiKey: 123 });
    assert.ok(error3);
    assert.ok(error3.includes('must be a string or null'));
  });

  it('validates bedrockRegion field through provider', function () {
    const provider = getProvider('claude');

    // Valid: null
    const error1 = provider.validateSettings({ bedrockRegion: null });
    assert.strictEqual(error1, null);

    // Valid: string
    const error2 = provider.validateSettings({ bedrockRegion: 'us-east-1' });
    assert.strictEqual(error2, null);

    // Invalid: not a string
    const error3 = provider.validateSettings({ bedrockRegion: 123 });
    assert.ok(error3);
    assert.ok(error3.includes('must be a string or null'));
  });

  it('validates complete Claude settings object', function () {
    const provider = getProvider('claude');

    // Valid complete settings
    const validSettings = {
      maxLevel: 'level3',
      minLevel: 'level1',
      defaultLevel: 'level2',
      levelOverrides: {},
      anthropicApiKey: 'sk-ant-test123',
      bedrockApiKey: 'bedrock-token',
      bedrockRegion: 'us-west-2',
    };

    const error = provider.validateSettings(validSettings);
    assert.strictEqual(error, null);
  });

  it('validates providerSettings through validateSetting', function () {
    // Valid: Claude with auth fields
    const validProviderSettings = {
      claude: {
        maxLevel: 'level3',
        minLevel: 'level1',
        defaultLevel: 'level2',
        levelOverrides: {},
        anthropicApiKey: 'sk-ant-test123',
        bedrockApiKey: null,
        bedrockRegion: null,
      },
      codex: {
        maxLevel: 'level3',
        minLevel: 'level1',
        defaultLevel: 'level2',
        levelOverrides: {},
      },
    };

    const error1 = validateSetting('providerSettings', validProviderSettings);
    assert.strictEqual(error1, null);

    // Invalid: Claude with bad anthropicApiKey
    const invalidProviderSettings = {
      claude: {
        maxLevel: 'level3',
        anthropicApiKey: 'bad-prefix',
      },
    };

    const error2 = validateSetting('providerSettings', invalidProviderSettings);
    assert.ok(error2);
    assert.ok(error2.includes('sk-ant-'));
  });

  it('ensures other providers do not have auth fields', function () {
    const codexProvider = getProvider('codex');
    const geminiProvider = getProvider('gemini');
    const opencodeProvider = getProvider('opencode');

    // Other providers should have basic fields only
    const codexDefaults = codexProvider.getDefaultSettings();
    assert.strictEqual(codexDefaults.anthropicApiKey, undefined);

    const geminiDefaults = geminiProvider.getDefaultSettings();
    assert.strictEqual(geminiDefaults.anthropicApiKey, undefined);

    const opencodeDefaults = opencodeProvider.getDefaultSettings();
    assert.strictEqual(opencodeDefaults.anthropicApiKey, undefined);
  });

  it('includes auth fields in Claude default settings', function () {
    const provider = getProvider('claude');
    const defaults = provider.getDefaultSettings();

    assert.strictEqual(defaults.anthropicApiKey, null);
    assert.strictEqual(defaults.bedrockApiKey, null);
    assert.strictEqual(defaults.bedrockRegion, null);
  });
});
