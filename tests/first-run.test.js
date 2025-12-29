/**
 * Test: First-Run Setup Wizard
 *
 * Tests first-time setup functionality
 * - Detection of first run
 * - Quiet mode (skip interactive prompts)
 * - Settings persistence after setup
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const assert = require('assert');

// Test storage directory (isolated)
const TEST_STORAGE_DIR = path.join(os.tmpdir(), 'zeroshot-first-run-test-' + Date.now());
const TEST_SETTINGS_FILE = path.join(TEST_STORAGE_DIR, 'settings.json');

// Set env var to use test settings file
// The settings module reads this dynamically via getSettingsFile()
process.env.ZEROSHOT_SETTINGS_FILE = TEST_SETTINGS_FILE;


const settingsModule = require('../lib/settings');
const firstRunModule = require('../cli/lib/first-run');

describe('First-Run Setup', function () {
  this.timeout(10000);

  before(function () {
    if (!fs.existsSync(TEST_STORAGE_DIR)) {
      fs.mkdirSync(TEST_STORAGE_DIR, { recursive: true });
    }
  });

  after(function () {
    // Clean up env var
    delete process.env.ZEROSHOT_SETTINGS_FILE;

    // Clean up test directory
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

  describe('detectFirstRun()', function () {
    it('should return true when firstRunComplete is false', function () {
      const settings = { firstRunComplete: false };
      assert.strictEqual(firstRunModule.detectFirstRun(settings), true);
    });

    it('should return true when firstRunComplete is undefined', function () {
      const settings = {};
      assert.strictEqual(firstRunModule.detectFirstRun(settings), true);
    });

    it('should return false when firstRunComplete is true', function () {
      const settings = { firstRunComplete: true };
      assert.strictEqual(firstRunModule.detectFirstRun(settings), false);
    });
  });

  describe('checkFirstRun() with quiet mode', function () {
    it('should return false when setup already complete', async function () {
      // Save settings with firstRunComplete = true
      const settings = {
        ...settingsModule.DEFAULT_SETTINGS,
        firstRunComplete: true,
      };
      fs.mkdirSync(path.dirname(TEST_SETTINGS_FILE), { recursive: true });
      fs.writeFileSync(TEST_SETTINGS_FILE, JSON.stringify(settings, null, 2), 'utf8');

      const wasSetupRun = await firstRunModule.checkFirstRun({ quiet: true });
      assert.strictEqual(wasSetupRun, false);
    });

    it('should return true and mark complete in quiet mode', async function () {
      // Start with no settings (first run)
      const wasSetupRun = await firstRunModule.checkFirstRun({ quiet: true });
      assert.strictEqual(wasSetupRun, true);

      // Verify settings were saved with firstRunComplete = true
      const savedSettings = JSON.parse(fs.readFileSync(TEST_SETTINGS_FILE, 'utf8'));
      assert.strictEqual(savedSettings.firstRunComplete, true);
    });

    it('should not prompt in quiet mode', async function () {
      // In quiet mode, checkFirstRun should use defaults and not require input
      const wasSetupRun = await firstRunModule.checkFirstRun({ quiet: true });
      assert.strictEqual(wasSetupRun, true);
    });
  });

  describe('printWelcome()', function () {
    it('should not throw when called', function () {
      assert.doesNotThrow(() => firstRunModule.printWelcome());
    });
  });

  describe('printComplete()', function () {
    it('should not throw when called with settings object', function () {
      const settings = {
        defaultModel: 'sonnet',
        autoCheckUpdates: true,
      };
      assert.doesNotThrow(() => firstRunModule.printComplete(settings));
    });
  });

  describe('Integration with settings', function () {
    it('should preserve other settings when marking setup complete', async function () {
      // Save initial settings with custom values
      const initialSettings = {
        ...settingsModule.DEFAULT_SETTINGS,
        defaultModel: 'haiku',
        logLevel: 'verbose',
        firstRunComplete: false,
      };
      fs.mkdirSync(path.dirname(TEST_SETTINGS_FILE), { recursive: true });
      fs.writeFileSync(TEST_SETTINGS_FILE, JSON.stringify(initialSettings, null, 2), 'utf8');

      // Run first-run in quiet mode
      await firstRunModule.checkFirstRun({ quiet: true });

      // Verify other settings were preserved
      const savedSettings = JSON.parse(fs.readFileSync(TEST_SETTINGS_FILE, 'utf8'));
      assert.strictEqual(savedSettings.firstRunComplete, true);
      assert.strictEqual(savedSettings.defaultModel, 'haiku');
      assert.strictEqual(savedSettings.logLevel, 'verbose');
    });
  });
});
