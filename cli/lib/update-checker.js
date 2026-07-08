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
const childProcess = require('child_process');
const fs = require('fs');
const path = require('path');
const readline = require('readline');
const { loadSettings, saveSettings } = require('../../lib/settings');

const NEW_PACKAGE_NAME = '@the-open-engine/zeroshot';
const LEGACY_PACKAGE_NAME = '@covibes/zeroshot';
const NEW_PACKAGE_SPEC = `${NEW_PACKAGE_NAME}@latest`;

// 24 hours in milliseconds
const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

// Timeout for npm registry fetch (5 seconds)
const FETCH_TIMEOUT_MS = 5000;

// npm registry URL
const REGISTRY_URL = `https://registry.npmjs.org/${NEW_PACKAGE_NAME}/latest`;

function getPackageMetadata() {
  return require('../../package.json');
}

function getCurrentPackageName() {
  return getPackageMetadata().name || NEW_PACKAGE_NAME;
}

/**
 * Get current package version
 * @returns {string}
 */
function getCurrentVersion() {
  return getPackageMetadata().version;
}

function isLegacyDistro(packageName = getCurrentPackageName()) {
  return packageName === LEGACY_PACKAGE_NAME;
}

function printLegacyDistroNotice(packageName = getCurrentPackageName()) {
  if (!isLegacyDistro(packageName)) {
    return false;
  }

  console.error(
    `\n⚠️  ${LEGACY_PACKAGE_NAME} has moved to ${NEW_PACKAGE_NAME}. ` +
      'Run `zeroshot update` to switch this installation.\n'
  );
  return true;
}

function getPackageRoot() {
  return path.dirname(require.resolve('../../package.json'));
}

function hasPathSuffix(parts, suffix) {
  if (suffix.length > parts.length) {
    return false;
  }

  const start = parts.length - suffix.length;
  return suffix.every((part, index) => parts[start + index] === part);
}

function joinPathParts(parts) {
  const joined = parts.join(path.sep);
  return joined === '' ? path.parse(process.cwd()).root : joined;
}

function deriveInstallPrefixFromPackageRoot(packageRoot, packageName) {
  const parts = path.resolve(packageRoot).split(path.sep);
  const packageParts = packageName.split('/');

  if (!hasPathSuffix(parts, packageParts)) {
    return null;
  }

  const nodeModulesIndex = parts.length - packageParts.length - 1;
  if (nodeModulesIndex < 0 || parts[nodeModulesIndex] !== 'node_modules') {
    return null;
  }

  if (parts[nodeModulesIndex - 1] === 'lib') {
    return joinPathParts(parts.slice(0, nodeModulesIndex - 1));
  }

  return joinPathParts(parts.slice(0, nodeModulesIndex));
}

function resolveNpmCommand(installPrefix = null) {
  const npmName = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  const candidates = [];

  if (installPrefix) {
    candidates.push(path.join(installPrefix, 'bin', npmName));
  }

  candidates.push(path.join(path.dirname(process.execPath), npmName));

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  return npmName;
}

function getNpmConfiguredPrefix(npmCommand = resolveNpmCommand()) {
  return childProcess
    .execFileSync(npmCommand, ['config', 'get', 'prefix'], {
      encoding: 'utf8',
    })
    .trim();
}

function getInstallPrefix(options = {}) {
  if (options.installPrefix) {
    return options.installPrefix;
  }

  const packageName = options.packageName || getCurrentPackageName();
  const packageRoot = options.packageRoot || getPackageRoot();
  const derivedPrefix = deriveInstallPrefixFromPackageRoot(packageRoot, packageName);

  if (derivedPrefix) {
    return derivedPrefix;
  }

  return getNpmConfiguredPrefix(options.npmCommand);
}

function getGlobalModulesDir(installPrefix) {
  const unixGlobalModulesDir = path.join(installPrefix, 'lib', 'node_modules');
  if (fs.existsSync(unixGlobalModulesDir)) {
    return unixGlobalModulesDir;
  }

  return path.join(installPrefix, 'node_modules');
}

function shellQuote(value) {
  if (/^[A-Za-z0-9_./:@+-]+$/.test(value)) {
    return value;
  }

  return `'${value.replace(/'/g, "'\\''")}'`;
}

function buildManualInstallCommand(installPrefix = null, useSudo = false) {
  const command = [
    useSudo ? 'sudo' : null,
    'npm',
    'install',
    '-g',
    installPrefix ? '--prefix' : null,
    installPrefix ? shellQuote(installPrefix) : null,
    NEW_PACKAGE_SPEC,
  ].filter(Boolean);

  return command.join(' ');
}

function getUpdateTarget(options = {}) {
  const packageName = options.packageName || getCurrentPackageName();
  const legacy = isLegacyDistro(packageName);
  const installPrefix = getInstallPrefix(options);
  const npmCommand = options.npmCommand || resolveNpmCommand(installPrefix);

  return {
    packageName,
    legacy,
    installPrefix,
    npmCommand,
    globalModulesDir: getGlobalModulesDir(installPrefix),
  };
}

function buildInstallArgs(updateTarget) {
  const args = ['install', '-g', '--prefix', updateTarget.installPrefix];

  if (updateTarget.legacy) {
    // npm refuses to replace the legacy package's `zeroshot` bin without this.
    args.push('--force');
  }

  args.push(NEW_PACKAGE_SPEC);
  return args;
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
function canWriteToNpmGlobal(options = {}) {
  try {
    const updateTarget = getUpdateTarget(options);
    fs.accessSync(updateTarget.globalModulesDir, fs.constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Run npm install to update the package
 * @returns {Promise<boolean>} True if update succeeded
 */
function runUpdate(options = {}) {
  return new Promise((resolve) => {
    let updateTarget;
    try {
      updateTarget = getUpdateTarget(options);
    } catch {
      console.log('❌ Update failed. Try manually:');
      console.log(`   ${buildManualInstallCommand()}\n`);
      resolve(false);
      return;
    }

    // Check permissions BEFORE attempting update
    if (!canWriteToNpmGlobal(options)) {
      console.log('\n⚠️  Cannot auto-update: no write permission to npm global directory.');
      console.log('   Run manually with sudo:');
      console.log(`   ${buildManualInstallCommand(updateTarget.installPrefix, true)}\n`);
      resolve(false);
      return;
    }

    console.log('\n📥 Installing update...');

    const proc = childProcess.spawn(updateTarget.npmCommand, buildInstallArgs(updateTarget), {
      stdio: 'inherit',
      shell: false,
    });

    proc.on('close', (code) => {
      if (code === 0) {
        console.log('✅ Update installed successfully!');
        console.log('   Restart zeroshot to use the new version.\n');
        resolve(true);
      } else {
        console.log('❌ Update failed. Try manually:');
        console.log(`   ${buildManualInstallCommand(updateTarget.installPrefix, true)}\n`);
        resolve(false);
      }
    });

    proc.on('error', () => {
      console.log('❌ Update failed. Try manually:');
      console.log(`   ${buildManualInstallCommand(updateTarget.installPrefix, true)}\n`);
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
      console.log(`   Run: ${buildManualInstallCommand(getInstallPrefix(), false)}\n`);
    } else {
      console.log(`   Run: ${buildManualInstallCommand(getInstallPrefix(), true)}\n`);
    }
    return;
  }

  // No write permission - inform user but don't offer interactive prompt
  // (they'd say yes then get an error, which is frustrating UX)
  if (!hasWriteAccess) {
    console.log(`\n📦 Update available: ${currentVersion} → ${latestVersion}`);
    console.log(`   Run: ${buildManualInstallCommand(getInstallPrefix(), true)}\n`);
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
  NEW_PACKAGE_NAME,
  LEGACY_PACKAGE_NAME,
  getCurrentVersion,
  getCurrentPackageName,
  isLegacyDistro,
  printLegacyDistroNotice,
  deriveInstallPrefixFromPackageRoot,
  getInstallPrefix,
  resolveNpmCommand,
  getUpdateTarget,
  buildInstallArgs,
  isNewerVersion,
  fetchLatestVersion,
  runUpdate,
  shouldCheckForUpdates,
  canWriteToNpmGlobal,
  CHECK_INTERVAL_MS,
};
