/**
 * Test: Update Checker
 *
 * Tests auto-update checking functionality
 * - Version comparison (semver)
 * - Check interval logic (24 hours)
 * - Settings persistence
 * - Quiet mode behavior
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const assert = require('assert');

// Test storage directory (isolated)
const TEST_STORAGE_DIR = path.join(os.tmpdir(), 'zeroshot-update-checker-test-' + Date.now());
const TEST_SETTINGS_FILE = path.join(TEST_STORAGE_DIR, 'settings.json');

describe('Update Checker', function () {
  this.timeout(10000);

  let updateChecker;
  let settingsModule;

  before(function () {
    if (!fs.existsSync(TEST_STORAGE_DIR)) {
      fs.mkdirSync(TEST_STORAGE_DIR, { recursive: true });
    }

    // Load settings module and override SETTINGS_FILE path
    settingsModule = require('../lib/settings');
    Object.defineProperty(settingsModule, 'SETTINGS_FILE', {
      value: TEST_SETTINGS_FILE,
      writable: false,
    });

    // Load update checker module
    updateChecker = require('../cli/lib/update-checker');
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

  describe('isNewerVersion()', function () {
    it('should return true when latest is newer (patch)', function () {
      assert.strictEqual(updateChecker.isNewerVersion('1.0.0', '1.0.1'), true);
    });

    it('should return true when latest is newer (minor)', function () {
      assert.strictEqual(updateChecker.isNewerVersion('1.0.0', '1.1.0'), true);
    });

    it('should return true when latest is newer (major)', function () {
      assert.strictEqual(updateChecker.isNewerVersion('1.0.0', '2.0.0'), true);
    });

    it('should return false when versions are equal', function () {
      assert.strictEqual(updateChecker.isNewerVersion('1.5.0', '1.5.0'), false);
    });

    it('should return false when current is newer', function () {
      assert.strictEqual(updateChecker.isNewerVersion('2.0.0', '1.5.0'), false);
    });

    it('should handle versions with different lengths', function () {
      assert.strictEqual(updateChecker.isNewerVersion('1.0', '1.0.1'), true);
      assert.strictEqual(updateChecker.isNewerVersion('1.0.0', '1.1'), true);
    });

    it('should handle complex version comparisons', function () {
      assert.strictEqual(updateChecker.isNewerVersion('1.9.9', '1.10.0'), true);
      assert.strictEqual(updateChecker.isNewerVersion('1.10.0', '1.9.9'), false);
      assert.strictEqual(updateChecker.isNewerVersion('0.9.9', '1.0.0'), true);
    });
  });

  describe('shouldCheckForUpdates()', function () {
    it('should return false when autoCheckUpdates is disabled', function () {
      const settings = {
        autoCheckUpdates: false,
        lastUpdateCheckAt: null,
      };
      assert.strictEqual(updateChecker.shouldCheckForUpdates(settings), false);
    });

    it('should return true when lastUpdateCheckAt is null (never checked)', function () {
      const settings = {
        autoCheckUpdates: true,
        lastUpdateCheckAt: null,
      };
      assert.strictEqual(updateChecker.shouldCheckForUpdates(settings), true);
    });

    it('should return false when less than 24 hours have passed', function () {
      const settings = {
        autoCheckUpdates: true,
        lastUpdateCheckAt: Date.now() - 12 * 60 * 60 * 1000, // 12 hours ago
      };
      assert.strictEqual(updateChecker.shouldCheckForUpdates(settings), false);
    });

    it('should return true when more than 24 hours have passed', function () {
      const settings = {
        autoCheckUpdates: true,
        lastUpdateCheckAt: Date.now() - 25 * 60 * 60 * 1000, // 25 hours ago
      };
      assert.strictEqual(updateChecker.shouldCheckForUpdates(settings), true);
    });

    it('should return true when exactly 24 hours have passed', function () {
      const settings = {
        autoCheckUpdates: true,
        lastUpdateCheckAt: Date.now() - 24 * 60 * 60 * 1000, // 24 hours ago
      };
      assert.strictEqual(updateChecker.shouldCheckForUpdates(settings), true);
    });
  });

  describe('getCurrentVersion()', function () {
    it('should return a valid version string', function () {
      const version = updateChecker.getCurrentVersion();
      assert.ok(typeof version === 'string');
      assert.ok(/^\d+\.\d+\.\d+/.test(version), `Version should be semver format: ${version}`);
    });
  });

  describe('CHECK_INTERVAL_MS', function () {
    it('should be 24 hours in milliseconds', function () {
      const expectedMs = 24 * 60 * 60 * 1000;
      assert.strictEqual(updateChecker.CHECK_INTERVAL_MS, expectedMs);
    });
  });

  describe('fetchLatestVersion()', function () {
    it('should return null or string (network dependent)', async function () {
      this.timeout(10000); // Network request may take time

      const version = await updateChecker.fetchLatestVersion();

      // Version should be null (network failure/timeout) or a valid semver string
      if (version !== null) {
        assert.ok(typeof version === 'string');
        assert.ok(/^\d+\.\d+\.\d+/.test(version), `Version should be semver format: ${version}`);
      }
    });
  });
});
