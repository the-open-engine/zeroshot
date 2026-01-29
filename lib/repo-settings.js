/**
 * Repo-local settings for zeroshot
 *
 * Optional per-repository config file:
 *   <repoRoot>/.zeroshot/settings.json
 *
 * This complements the global user settings at:
 *   ~/.zeroshot/settings.json
 */

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

function _safeJsonParse(text) {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function _getGitRoot(dir) {
  try {
    return execSync('git rev-parse --show-toplevel', {
      cwd: dir,
      encoding: 'utf8',
      stdio: 'pipe',
    }).trim();
  } catch {
    return null;
  }
}

function _readSettingsFile(filePath) {
  try {
    const raw = fs.readFileSync(filePath, 'utf8');
    const parsed = _safeJsonParse(raw);
    if (!parsed || typeof parsed !== 'object') {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/**
 * Read repo-local settings if present.
 *
 * @param {string} startDir - Directory inside the repo (usually process.cwd()).
 * @returns {{repoRoot: string|null, settings: object|null, settingsPath: string|null}}
 */
function readRepoSettings(startDir) {
  const repoRoot = _getGitRoot(startDir);
  if (!repoRoot) {
    return { repoRoot: null, settings: null, settingsPath: null };
  }

  const settingsPath = path.join(repoRoot, '.zeroshot', 'settings.json');
  if (!fs.existsSync(settingsPath)) {
    return { repoRoot, settings: null, settingsPath };
  }

  const settings = _readSettingsFile(settingsPath);
  return { repoRoot, settings, settingsPath };
}

module.exports = { readRepoSettings };
