const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const { loadSettings, validateSetting } = require('../lib/settings');
const { validateProviderSettings, validateProviderLevel } = require('../src/config-validator');
const { getProvider } = require('../src/providers');

describe('Provider settings', function () {
  const testDir = path.join(os.tmpdir(), `zeroshot-provider-settings-${Date.now()}`);
  const settingsFile = path.join(testDir, 'settings.json');

  before(function () {
    fs.mkdirSync(testDir, { recursive: true });
  });

  after(function () {
    delete process.env.ZEROSHOT_SETTINGS_FILE;
    try {
      fs.rmSync(testDir, { recursive: true, force: true });
    } catch {
      // ignore cleanup errors
    }
  });

  it('validates defaultProvider values (including legacy aliases)', function () {
    assert.strictEqual(validateSetting('defaultProvider', 'codex'), null);
    assert.strictEqual(validateSetting('defaultProvider', 'openai'), null);
    assert.strictEqual(validateSetting('defaultProvider', 'opencode'), null);
    const error = validateSetting('defaultProvider', 'invalid-provider');
    assert.ok(error);
  });

  it('validates provider level bounds', function () {
    assert.doesNotThrow(() => {
      validateProviderLevel('codex', 'level2', 'level1', 'level3');
    });

    assert.throws(() => {
      validateProviderLevel('codex', 'level4', 'level1', 'level3');
    }, /Invalid level/);
  });

  it('validates provider overrides and reasoning rules', function () {
    assert.doesNotThrow(() => {
      validateProviderSettings('codex', {
        minLevel: 'level1',
        maxLevel: 'level3',
        defaultLevel: 'level2',
        levelOverrides: {
          level1: { model: 'gpt-5.4', reasoningEffort: 'low' },
        },
      });
    });

    assert.doesNotThrow(() => {
      validateProviderSettings('opencode', {
        minLevel: 'level1',
        maxLevel: 'level3',
        defaultLevel: 'level2',
        levelOverrides: {
          level2: { reasoningEffort: 'high' },
        },
      });
    });

    assert.throws(() => {
      validateProviderSettings('gemini', {
        minLevel: 'level1',
        maxLevel: 'level3',
        defaultLevel: 'level2',
        levelOverrides: {
          level2: { reasoningEffort: 'high' },
        },
      });
    }, /reasoningEffort overrides are only supported/);
  });

  it('applies legacy maxModel to claude levels', function () {
    process.env.ZEROSHOT_SETTINGS_FILE = settingsFile;
    fs.writeFileSync(settingsFile, JSON.stringify({ maxModel: 'haiku' }, null, 2), 'utf8');

    const settings = loadSettings();
    assert.strictEqual(settings.providerSettings.claude.maxLevel, 'level1');
    assert.strictEqual(settings.providerSettings.claude.defaultLevel, 'level1');
  });

  it('uses gpt-5.4 as the default codex model', function () {
    const codex = getProvider('codex');
    const modelSpec = codex.resolveModelSpec(codex.getDefaultLevel(), {});
    assert.strictEqual(modelSpec.model, 'gpt-5.4');
  });

  it('maps claude level3 to opus alias', function () {
    const claude = getProvider('claude');
    const modelSpec = claude.resolveModelSpec('level3', {});
    assert.strictEqual(modelSpec.model, 'opus');
  });

  it('rejects legacy claude model aliases as invalid input', function () {
    const claude = getProvider('claude');
    assert.throws(() => {
      claude.validateModelId('opus-4.6');
    }, /Use canonical model ids: haiku, sonnet, opus/);
  });

  it('marks invalid model errors as permanent', function () {
    const claude = getProvider('claude');
    assert.throws(() => {
      try {
        claude.validateModelId('not-a-model');
      } catch (error) {
        assert.strictEqual(error.permanent, true);
        throw error;
      }
    }, /Invalid model "not-a-model"/);
  });

  it('fails before command build when model override is invalid', function () {
    const claude = getProvider('claude');
    assert.throws(() => {
      claude.buildCommand('test context', {
        modelSpec: { model: 'opus-4.6' },
        cliFeatures: { supportsModel: true },
      });
    }, /Use canonical model ids: haiku, sonnet, opus/);
  });
});
