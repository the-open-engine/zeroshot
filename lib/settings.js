/**
 * Settings management for zeroshot
 * Persistent user preferences stored in ~/.zeroshot/settings.json
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const { validateMountConfig, validateEnvPassthrough } = require('./docker-config');

/**
 * Get settings file path (dynamically reads env var for testing)
 * Using a getter ensures tests can override the path at runtime
 * @returns {string}
 */
function getSettingsFile() {
  return process.env.ZEROSHOT_SETTINGS_FILE || path.join(os.homedir(), '.zeroshot', 'settings.json');
}

/**
 * Model hierarchy for cost ceiling validation
 * Higher number = more expensive/capable model
 */
const MODEL_HIERARCHY = {
  opus: 3,
  sonnet: 2,
  haiku: 1,
};

const VALID_MODELS = Object.keys(MODEL_HIERARCHY);

/**
 * Validate a requested model against the maxModel ceiling
 * @param {string} requestedModel - Model the agent wants to use
 * @param {string} maxModel - Maximum allowed model (cost ceiling)
 * @returns {string} The validated model
 * @throws {Error} If requested model exceeds ceiling
 */
function validateModelAgainstMax(requestedModel, maxModel) {
  if (!requestedModel) return maxModel; // Default to ceiling if unspecified

  if (!VALID_MODELS.includes(requestedModel)) {
    throw new Error(`Invalid model "${requestedModel}". Valid: ${VALID_MODELS.join(', ')}`);
  }
  if (!VALID_MODELS.includes(maxModel)) {
    throw new Error(`Invalid maxModel "${maxModel}". Valid: ${VALID_MODELS.join(', ')}`);
  }

  if (MODEL_HIERARCHY[requestedModel] > MODEL_HIERARCHY[maxModel]) {
    throw new Error(
      `Agent requests "${requestedModel}" but maxModel is "${maxModel}". ` +
        `Either lower agent's model or raise maxModel.`
    );
  }
  return requestedModel;
}

// Default settings
const DEFAULT_SETTINGS = {
  maxModel: 'opus', // Cost ceiling - agents cannot use models above this
  defaultConfig: 'conductor-bootstrap',
  defaultDocker: false,
  strictSchema: true, // true = reliable json output (default), false = live streaming (may crash - see bold-meadow-11)
  logLevel: 'normal',
  // Auto-update settings
  autoCheckUpdates: true, // Check npm registry for newer versions
  lastUpdateCheckAt: null, // Unix timestamp of last check (null = never checked)
  lastSeenVersion: null, // Don't re-prompt for same version
  // Claude command - customize how to invoke Claude CLI (default: 'claude')
  // Example: 'ccr code' for claude-code-router integration
  claudeCommand: 'claude',
  // Docker isolation mounts - preset names or {host, container, readonly?} objects
  // Valid presets: gh, git, ssh, aws, azure, kube, terraform, gcloud
  dockerMounts: ['gh', 'git', 'ssh'],
  // Extra env vars to pass to Docker container (in addition to preset-implied ones)
  // Supports: VAR (if set), VAR_* (pattern), VAR=value (forced), VAR= (empty)
  dockerEnvPassthrough: [],
  // Container home directory - where $HOME resolves in container paths
  // Default: /home/node (matches zeroshot-cluster-base image)
  dockerContainerHome: '/home/node',
};

/**
 * Load settings from disk, merging with defaults
 */
function loadSettings() {
  const settingsFile = getSettingsFile();
  if (!fs.existsSync(settingsFile)) {
    return { ...DEFAULT_SETTINGS };
  }
  try {
    const data = fs.readFileSync(settingsFile, 'utf8');
    return { ...DEFAULT_SETTINGS, ...JSON.parse(data) };
  } catch {
    console.error('Warning: Could not load settings, using defaults');
    return { ...DEFAULT_SETTINGS };
  }
}

/**
 * Save settings to disk
 */
function saveSettings(settings) {
  const settingsFile = getSettingsFile();
  const dir = path.dirname(settingsFile);
  if (!fs.existsSync(dir)) {
    fs.mkdirSync(dir, { recursive: true });
  }
  fs.writeFileSync(settingsFile, JSON.stringify(settings, null, 2), 'utf8');
}

/**
 * Validate a setting value
 * @returns {string|null} Error message if invalid, null if valid
 */
function validateSetting(key, value) {
  if (!(key in DEFAULT_SETTINGS)) {
    return `Unknown setting: ${key}`;
  }

  if (key === 'maxModel' && !VALID_MODELS.includes(value)) {
    return `Invalid model: ${value}. Valid models: ${VALID_MODELS.join(', ')}`;
  }

  if (key === 'logLevel' && !['quiet', 'normal', 'verbose'].includes(value)) {
    return `Invalid log level: ${value}. Valid levels: quiet, normal, verbose`;
  }

  if (key === 'claudeCommand') {
    if (typeof value !== 'string') {
      return 'claudeCommand must be a string';
    }
    if (value.trim().length === 0) {
      return 'claudeCommand cannot be empty';
    }
  }

  if (key === 'dockerMounts') {
    return validateMountConfig(value);
  }

  if (key === 'dockerEnvPassthrough') {
    return validateEnvPassthrough(value);
  }

  return null;
}

/**
 * Coerce value to correct type based on default value type
 */
function coerceValue(key, value) {
  const defaultValue = DEFAULT_SETTINGS[key];

  if (typeof defaultValue === 'boolean') {
    return value === 'true' || value === '1' || value === 'yes' || value === true;
  }

  if (typeof defaultValue === 'number') {
    const parsed = parseInt(value);
    if (isNaN(parsed)) {
      throw new Error(`Invalid number: ${value}`);
    }
    return parsed;
  }

  // Handle array settings (dockerMounts, dockerEnvPassthrough)
  if (Array.isArray(defaultValue)) {
    if (typeof value === 'string') {
      try {
        const parsed = JSON.parse(value);
        if (!Array.isArray(parsed)) {
          throw new Error(`${key} must be an array`);
        }
        return parsed;
      } catch (e) {
        if (e instanceof SyntaxError) {
          throw new Error(`Invalid JSON for ${key}: ${value}`);
        }
        throw e;
      }
    }
    return value;
  }

  return value;
}

/**
 * Get parsed Claude command from settings/env
 * Supports space-separated commands like 'ccr code'
 * @returns {{ command: string, args: string[] }}
 */
function getClaudeCommand() {
  const settings = loadSettings();
  const raw = process.env.ZEROSHOT_CLAUDE_COMMAND || settings.claudeCommand || 'claude';
  const parts = raw.trim().split(/\s+/);
  return {
    command: parts[0],
    args: parts.slice(1),
  };
}

module.exports = {
  loadSettings,
  saveSettings,
  validateSetting,
  coerceValue,
  DEFAULT_SETTINGS,
  getSettingsFile,
  getClaudeCommand,
  // Model validation exports
  MODEL_HIERARCHY,
  VALID_MODELS,
  validateModelAgainstMax,
  // Backward compatibility: SETTINGS_FILE as getter (reads env var dynamically)
  get SETTINGS_FILE() {
    return getSettingsFile();
  },
};
