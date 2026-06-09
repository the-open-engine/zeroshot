const fs = require('fs');
const os = require('os');
const path = require('path');

const { resolveWorktreeRoot } = require('./worktree-tooling-env');

const CLAUDE_DIRNAME = '.claude';
const SETTINGS_BASENAME = 'settings.json';
const MCP_BASENAME = '.mcp.json';

function isPlainObject(value) {
  return value && typeof value === 'object' && !Array.isArray(value);
}

function readJsonIfExists(filePath) {
  if (!fs.existsSync(filePath)) {
    return null;
  }

  try {
    return JSON.parse(fs.readFileSync(filePath, 'utf8'));
  } catch {
    return null;
  }
}

function mergeJson(baseValue, overrideValue) {
  if (Array.isArray(baseValue) && Array.isArray(overrideValue)) {
    const seen = new Set();
    const merged = [];

    for (const entry of [...baseValue, ...overrideValue]) {
      const key = JSON.stringify(entry);
      if (seen.has(key)) {
        continue;
      }
      seen.add(key);
      merged.push(entry);
    }

    return merged;
  }

  if (isPlainObject(baseValue) && isPlainObject(overrideValue)) {
    const merged = { ...baseValue };
    for (const [key, value] of Object.entries(overrideValue)) {
      if (Object.prototype.hasOwnProperty.call(merged, key)) {
        merged[key] = mergeJson(merged[key], value);
      } else {
        merged[key] = value;
      }
    }
    return merged;
  }

  return overrideValue === undefined ? baseValue : overrideValue;
}

function ensureDir(dirPath) {
  fs.mkdirSync(dirPath, { recursive: true });
}

function copyIfExists(sourcePath, destPath) {
  if (!fs.existsSync(sourcePath)) {
    return;
  }

  ensureDir(path.dirname(destPath));
  fs.copyFileSync(sourcePath, destPath);
}

function resolveRepoClaudeConfig(worktreeRoot) {
  const configDir = path.join(worktreeRoot, CLAUDE_DIRNAME);
  const settingsPath = path.join(configDir, SETTINGS_BASENAME);
  const mcpPath = path.join(configDir, MCP_BASENAME);

  if (!fs.existsSync(settingsPath) && !fs.existsSync(mcpPath)) {
    return null;
  }

  return {
    configDir,
    settingsPath,
    mcpPath,
  };
}

function prepareClaudeConfigDir(options = {}) {
  const worktreeRoot = resolveWorktreeRoot(options.worktreePath || options.cwd);
  if (!worktreeRoot) {
    return null;
  }

  const repoConfig = resolveRepoClaudeConfig(worktreeRoot);
  if (!repoConfig) {
    return null;
  }

  const sourceDir =
    options.sourceDir || process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), CLAUDE_DIRNAME);
  const tempRoot = path.join(os.tmpdir(), 'zeroshot-claude-configs');
  ensureDir(tempRoot);

  const overlayDir = fs.mkdtempSync(path.join(tempRoot, 'config-'));
  ensureDir(path.join(overlayDir, 'hooks'));
  ensureDir(path.join(overlayDir, 'projects'));

  copyIfExists(
    path.join(sourceDir, '.credentials.json'),
    path.join(overlayDir, '.credentials.json')
  );

  const sourceSettings = readJsonIfExists(path.join(sourceDir, SETTINGS_BASENAME)) || {};
  const repoSettings = readJsonIfExists(repoConfig.settingsPath) || {};
  const mergedSettings = mergeJson(sourceSettings, repoSettings);
  if (Object.keys(mergedSettings).length > 0) {
    fs.writeFileSync(
      path.join(overlayDir, SETTINGS_BASENAME),
      JSON.stringify(mergedSettings, null, 2)
    );
  }

  const sourceMcp = readJsonIfExists(path.join(sourceDir, MCP_BASENAME)) || {};
  const repoMcp = readJsonIfExists(repoConfig.mcpPath) || {};
  const mergedMcp = mergeJson(sourceMcp, repoMcp);
  if (Object.keys(mergedMcp).length > 0) {
    fs.writeFileSync(path.join(overlayDir, MCP_BASENAME), JSON.stringify(mergedMcp, null, 2));
  }

  return overlayDir;
}

module.exports = {
  prepareClaudeConfigDir,
};
