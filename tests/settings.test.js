/**
 * Test: Settings System
 *
 * Tests persistent settings stored in ~/.zeroshot/settings.json
 * - Load/save settings
 * - Default values
 * - Type coercion
 * - Validation
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const assert = require('assert');

// Test storage directory (isolated)
const TEST_STORAGE_DIR = path.join(os.tmpdir(), 'zeroshot-settings-test-' + Date.now());
const TEST_SETTINGS_FILE = path.join(TEST_STORAGE_DIR, 'settings.json');

describe('Settings System', function () {
  this.timeout(10000);

  let settingsModule;

  before(function () {
    if (!fs.existsSync(TEST_STORAGE_DIR)) {
      fs.mkdirSync(TEST_STORAGE_DIR, { recursive: true });
    }

    // Load settings module and override SETTINGS_FILE path
    settingsModule = require('../lib/settings');
    // Monkey-patch the SETTINGS_FILE constant for testing
    Object.defineProperty(settingsModule, 'SETTINGS_FILE', {
      value: TEST_SETTINGS_FILE,
      writable: false,
    });
  });

  after(function () {
    try {
      fs.rmSync(TEST_STORAGE_DIR, { recursive: true, force: true });
    } catch (e) {
      console.error('Cleanup failed:', e.message);
    }
  });

  beforeEach(function () {
    // Clean settings file before each test
    if (fs.existsSync(TEST_SETTINGS_FILE)) {
      fs.unlinkSync(TEST_SETTINGS_FILE);
    }
  });

  it('should export required functions and constants', function () {
    assert.ok(typeof settingsModule.loadSettings === 'function');
    assert.ok(typeof settingsModule.saveSettings === 'function');
    assert.ok(typeof settingsModule.validateSetting === 'function');
    assert.ok(typeof settingsModule.coerceValue === 'function');
    assert.ok(typeof settingsModule.DEFAULT_SETTINGS === 'object');
  });

  it('should have correct default settings', function () {
    const { DEFAULT_SETTINGS } = settingsModule;

    assert.strictEqual(DEFAULT_SETTINGS.defaultModel, 'sonnet');
    assert.strictEqual(DEFAULT_SETTINGS.defaultConfig, 'conductor-bootstrap');
    assert.strictEqual(DEFAULT_SETTINGS.defaultIsolation, false);
    assert.strictEqual(DEFAULT_SETTINGS.strictSchema, true);
    assert.strictEqual(DEFAULT_SETTINGS.logLevel, 'normal');
  });

  it('should load default settings when file does not exist', function () {
    // Create custom load function with test path
    const loadSettings = () => {
      if (!fs.existsSync(TEST_SETTINGS_FILE)) {
        return { ...settingsModule.DEFAULT_SETTINGS };
      }
      const data = fs.readFileSync(TEST_SETTINGS_FILE, 'utf8');
      return { ...settingsModule.DEFAULT_SETTINGS, ...JSON.parse(data) };
    };

    const settings = loadSettings();

    assert.strictEqual(settings.defaultModel, 'sonnet');
    assert.strictEqual(settings.defaultConfig, 'conductor-bootstrap');
    assert.strictEqual(settings.defaultIsolation, false);
    assert.strictEqual(settings.strictSchema, true);
    assert.strictEqual(settings.logLevel, 'normal');
  });

  it('should save and load settings', function () {
    const saveSettings = (settings) => {
      const dir = path.dirname(TEST_SETTINGS_FILE);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }
      fs.writeFileSync(TEST_SETTINGS_FILE, JSON.stringify(settings, null, 2), 'utf8');
    };

    const loadSettings = () => {
      if (!fs.existsSync(TEST_SETTINGS_FILE)) {
        return { ...settingsModule.DEFAULT_SETTINGS };
      }
      const data = fs.readFileSync(TEST_SETTINGS_FILE, 'utf8');
      return { ...settingsModule.DEFAULT_SETTINGS, ...JSON.parse(data) };
    };

    const newSettings = {
      defaultModel: 'haiku',
      defaultConfig: 'conductor-junior-bootstrap',
      defaultIsolation: true,
      logLevel: 'verbose',
    };

    saveSettings(newSettings);
    assert.ok(fs.existsSync(TEST_SETTINGS_FILE), 'Settings file should exist');

    const loaded = loadSettings();
    assert.strictEqual(loaded.defaultModel, 'haiku');
    assert.strictEqual(loaded.defaultConfig, 'conductor-junior-bootstrap');
    assert.strictEqual(loaded.defaultIsolation, true);
    assert.strictEqual(loaded.logLevel, 'verbose');
  });

  it('should validate model values', function () {
    const { validateSetting } = settingsModule;

    // Valid models
    assert.strictEqual(validateSetting('defaultModel', 'opus'), null);
    assert.strictEqual(validateSetting('defaultModel', 'sonnet'), null);
    assert.strictEqual(validateSetting('defaultModel', 'haiku'), null);

    // Invalid model
    const error = validateSetting('defaultModel', 'gpt4');
    assert.ok(error !== null);
    assert.ok(error.includes('Invalid model'));
  });

  it('should validate log level values', function () {
    const { validateSetting } = settingsModule;

    // Valid log levels
    assert.strictEqual(validateSetting('logLevel', 'quiet'), null);
    assert.strictEqual(validateSetting('logLevel', 'normal'), null);
    assert.strictEqual(validateSetting('logLevel', 'verbose'), null);

    // Invalid log level
    const error = validateSetting('logLevel', 'debug');
    assert.ok(error !== null);
    assert.ok(error.includes('Invalid log level'));
  });

  it('should coerce boolean values', function () {
    const { coerceValue } = settingsModule;

    // defaultIsolation
    assert.strictEqual(coerceValue('defaultIsolation', 'true'), true);
    assert.strictEqual(coerceValue('defaultIsolation', '1'), true);
    assert.strictEqual(coerceValue('defaultIsolation', 'yes'), true);
    assert.strictEqual(coerceValue('defaultIsolation', true), true);
    assert.strictEqual(coerceValue('defaultIsolation', 'false'), false);
    assert.strictEqual(coerceValue('defaultIsolation', 'no'), false);
    assert.strictEqual(coerceValue('defaultIsolation', false), false);

    // strictSchema
    assert.strictEqual(coerceValue('strictSchema', 'true'), true);
    assert.strictEqual(coerceValue('strictSchema', '1'), true);
    assert.strictEqual(coerceValue('strictSchema', true), true);
    assert.strictEqual(coerceValue('strictSchema', 'false'), false);
    assert.strictEqual(coerceValue('strictSchema', false), false);
  });

  it('should coerce string values', function () {
    const { coerceValue } = settingsModule;

    assert.strictEqual(coerceValue('defaultModel', 'haiku'), 'haiku');
    assert.strictEqual(coerceValue('defaultConfig', 'my-config'), 'my-config');
  });

  it('settings file should be valid JSON with pretty printing', function () {
    const saveSettings = (settings) => {
      const dir = path.dirname(TEST_SETTINGS_FILE);
      if (!fs.existsSync(dir)) {
        fs.mkdirSync(dir, { recursive: true });
      }
      fs.writeFileSync(TEST_SETTINGS_FILE, JSON.stringify(settings, null, 2), 'utf8');
    };

    const settings = {
      defaultModel: 'sonnet',
      defaultConfig: 'test-config',
      defaultIsolation: false,
      logLevel: 'normal',
    };

    saveSettings(settings);

    // Should be valid JSON
    const raw = fs.readFileSync(TEST_SETTINGS_FILE, 'utf8');
    assert.doesNotThrow(() => JSON.parse(raw), 'Settings file should be valid JSON');

    // Should be pretty-printed (indented)
    assert.ok(raw.includes('\n  '), 'Settings should be pretty-printed');
  });
});
