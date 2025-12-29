/**
 * Update Checker - Checks npm registry for newer versions
 *
 * Features:
 * - 24-hour check interval (avoids registry spam)
 * - 5-second timeout (non-blocking if offline)
 * - Interactive prompt for manual update
 * - Respects quiet mode (no prompts in CI/scripts)
 */

const https = require('https');
const { spawn } = require('child_process');
const readline = require('readline');
const { loadSettings, saveSettings } = require('../../lib/settings');

// 24 hours in milliseconds
const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

// Timeout for npm registry fetch (5 seconds)
const FETCH_TIMEOUT_MS = 5000;

// npm registry URL
const REGISTRY_URL = 'https://registry.npmjs.org/@covibes/zeroshot/latest';

/**
 * Get current package version
 * @returns {string}
 */
function getCurrentVersion() {
  const pkg = require('../../package.json');
  return pkg.version;
}

/**
 * Compare semver versions
 * @param {string} current - Current version (e.g., "1.5.0")
 * @param {string} latest - Latest version (e.g., "1.6.0")
 * @returns {boolean} True if latest > current
 */
function isNewerVersion(current, latest) {
  const currentParts = current.split('.').map(Number);
  const latestParts = latest.split('.').map(Number);

  for (let i = 0; i < 3; i++) {
    const c = currentParts[i] || 0;
    const l = latestParts[i] || 0;
    if (l > c) return true;
    if (l < c) return false;
  }
  return false;
}

/**
 * Fetch latest version from npm registry
 * @returns {Promise<string|null>} Latest version or null on failure
 */
function fetchLatestVersion() {
  return new Promise((resolve) => {
    const req = https.get(REGISTRY_URL, { timeout: FETCH_TIMEOUT_MS }, (res) => {
      if (res.statusCode !== 200) {
        resolve(null);
        return;
      }

      let data = '';
      res.on('data', (chunk) => {
        data += chunk;
      });

      res.on('end', () => {
        try {
          const json = JSON.parse(data);
          resolve(json.version || null);
        } catch {
          resolve(null);
        }
      });
    });

    req.on('error', () => {
      resolve(null);
    });

    req.on('timeout', () => {
      req.destroy();
      resolve(null);
    });

    // Additional safety timeout
    setTimeout(() => {
      req.destroy();
      resolve(null);
    }, FETCH_TIMEOUT_MS + 1000);
  });
}

/**
 * Prompt user for update confirmation
 * @param {string} currentVersion
 * @param {string} latestVersion
 * @returns {Promise<boolean>} True if user wants to update
 */
function promptForUpdate(currentVersion, latestVersion) {
  return new Promise((resolve) => {
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });

    console.log(`\n📦 Update available: ${currentVersion} → ${latestVersion}`);
    rl.question('   Install now? [y/N] ', (answer) => {
      rl.close();
      resolve(answer.toLowerCase() === 'y' || answer.toLowerCase() === 'yes');
    });
  });
}

/**
 * Check if we have write permission to npm global directory
 * @returns {boolean} True if we can write to npm global prefix
 */
function canWriteToNpmGlobal() {
  const { execSync } = require('child_process');
  const fs = require('fs');

  try {
    // Get npm global prefix (e.g., /usr/lib or /home/user/.nvm/versions/node/...)
    const prefix = execSync('npm config get prefix', { encoding: 'utf8' }).trim();
    const globalModulesDir = require('path').join(prefix, 'lib', 'node_modules');

    // Check if directory exists and is writable
    fs.accessSync(globalModulesDir, fs.constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Run npm install to update the package
 * @returns {Promise<boolean>} True if update succeeded
 */
function runUpdate() {
  return new Promise((resolve) => {
    // Check permissions BEFORE attempting update
    if (!canWriteToNpmGlobal()) {
      console.log('\n⚠️  Cannot auto-update: no write permission to npm global directory.');
      console.log('   Run manually with sudo:');
      console.log('   sudo npm install -g @covibes/zeroshot@latest\n');
      resolve(false);
      return;
    }

    console.log('\n📥 Installing update...');

    const proc = spawn('npm', ['install', '-g', '@covibes/zeroshot@latest'], {
      stdio: 'inherit',
      shell: true,
    });

    proc.on('close', (code) => {
      if (code === 0) {
        console.log('✅ Update installed successfully!');
        console.log('   Restart zeroshot to use the new version.\n');
        resolve(true);
      } else {
        console.log('❌ Update failed. Try manually:');
        console.log('   sudo npm install -g @covibes/zeroshot@latest\n');
        resolve(false);
      }
    });

    proc.on('error', () => {
      console.log('❌ Update failed. Try manually:');
      console.log('   sudo npm install -g @covibes/zeroshot@latest\n');
      resolve(false);
    });
  });
}

/**
 * Check if update check should run
 * @param {object} settings - Current settings
 * @returns {boolean}
 */
function shouldCheckForUpdates(settings) {
  // Disabled by user
  if (!settings.autoCheckUpdates) {
    return false;
  }

  // Never checked before
  if (!settings.lastUpdateCheckAt) {
    return true;
  }

  // Check if 24 hours have passed
  const elapsed = Date.now() - settings.lastUpdateCheckAt;
  return elapsed >= CHECK_INTERVAL_MS;
}

/**
 * Main entry point - check for updates
 * @param {object} options
 * @param {boolean} options.quiet - Skip interactive prompts
 * @returns {Promise<void>}
 */
async function checkForUpdates(options = {}) {
  const settings = loadSettings();

  // Skip check if not due
  if (!shouldCheckForUpdates(settings)) {
    return;
  }

  const currentVersion = getCurrentVersion();
  const latestVersion = await fetchLatestVersion();

  // Update last check timestamp regardless of result
  settings.lastUpdateCheckAt = Date.now();
  saveSettings(settings);

  // Network failure - silently skip
  if (!latestVersion) {
    return;
  }

  // No update available
  if (!isNewerVersion(currentVersion, latestVersion)) {
    return;
  }

  // Already notified about this version
  if (settings.lastSeenVersion === latestVersion) {
    return;
  }

  // Update lastSeenVersion so we don't nag about the same version
  settings.lastSeenVersion = latestVersion;
  saveSettings(settings);

  // Check write permissions upfront
  const hasWriteAccess = canWriteToNpmGlobal();

  // Quiet mode - just inform, no prompt
  if (options.quiet) {
    console.log(`📦 Update available: ${currentVersion} → ${latestVersion}`);
    if (hasWriteAccess) {
      console.log('   Run: npm install -g @covibes/zeroshot@latest\n');
    } else {
      console.log('   Run: sudo npm install -g @covibes/zeroshot@latest\n');
    }
    return;
  }

  // No write permission - inform user but don't offer interactive prompt
  // (they'd say yes then get an error, which is frustrating UX)
  if (!hasWriteAccess) {
    console.log(`\n📦 Update available: ${currentVersion} → ${latestVersion}`);
    console.log('   Run: sudo npm install -g @covibes/zeroshot@latest\n');
    return;
  }

  // Interactive mode - prompt for update (only if we have write access)
  const wantsUpdate = await promptForUpdate(currentVersion, latestVersion);
  if (wantsUpdate) {
    await runUpdate();
  }
}

module.exports = {
  checkForUpdates,
  // Exported for testing and CLI update command
  getCurrentVersion,
  isNewerVersion,
  fetchLatestVersion,
  runUpdate,
  shouldCheckForUpdates,
  canWriteToNpmGlobal,
  CHECK_INTERVAL_MS,
};
