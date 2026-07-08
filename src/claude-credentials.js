/**
 * Provisions Claude Code credentials into an isolated CLAUDE_CONFIG_DIR.
 *
 * Claude Code only reads the macOS Keychain when using the DEFAULT config dir.
 * Isolated agent runs (--worktree, --docker) use a custom CLAUDE_CONFIG_DIR, so
 * a Keychain-only (OAuth subscription) login must be materialized to a file or
 * every isolated agent fails auth even though the host is logged in.
 */

const fs = require('fs');
const os = require('os');
const path = require('path');
const { execSync } = require('./lib/safe-exec');

const CREDENTIALS_BASENAME = '.credentials.json';
const KEYCHAIN_SERVICE = 'Claude Code-credentials';

/**
 * Read the Claude Code OAuth credentials JSON from the macOS Keychain.
 * @returns {string|null} Raw credentials JSON, or null if unavailable/invalid/non-darwin.
 */
function readKeychainCredentials() {
  if (os.platform() !== 'darwin') {
    return null;
  }

  try {
    const output = execSync(`security find-generic-password -s "${KEYCHAIN_SERVICE}" -w`, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
      timeout: 2000,
    });
    const trimmed = output.trim();
    if (!trimmed) {
      return null;
    }
    JSON.parse(trimmed); // reject garbage output that isn't real credentials
    return trimmed;
  } catch {
    return null;
  }
}

/**
 * Copy an existing credentials file into an isolated config dir, or materialize
 * one from the macOS Keychain when no source file exists. Resynced on every call
 * so an expired-then-refreshed OAuth token in the Keychain is picked up per run.
 * @param {{ sourceDir: string, destDir: string }} options
 * @returns {boolean} true if a credentials file was written into destDir
 */
function provisionClaudeCredentials({ sourceDir, destDir }) {
  fs.mkdirSync(destDir, { recursive: true });

  const destPath = path.join(destDir, CREDENTIALS_BASENAME);
  const sourcePath = path.join(sourceDir, CREDENTIALS_BASENAME);

  if (fs.existsSync(sourcePath)) {
    fs.copyFileSync(sourcePath, destPath);
    fs.chmodSync(destPath, 0o600);
    return true;
  }

  const keychainCredentials = readKeychainCredentials();
  if (keychainCredentials) {
    fs.writeFileSync(destPath, keychainCredentials, { mode: 0o600 });
    return true;
  }

  return false;
}

module.exports = {
  readKeychainCredentials,
  provisionClaudeCredentials,
};
