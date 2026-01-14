/**
 * Settings management for zeroshot
 * Persistent user preferences stored in ~/.zeroshot/settings.json
 */

const fs = require('fs');
const path = require('path');
const os = require('os');
const { validateMountConfig, validateEnvPassthrough } = require('./docker-config');
const {
  VALID_PROVIDERS,
  normalizeProviderName,
  normalizeProviderSettings,
} = require('./provider-names');

/**
 * Get settings file path (dynamically reads env var for testing)
 * Using a getter ensures tests can override the path at runtime
 * @returns {string}
 */
function getSettingsFile() {
  return (
    process.env.ZEROSHOT_SETTINGS_FILE || path.join(os.homedir(), '.zeroshot', 'settings.json')
  );
}

// Import provider defaults from separate module to avoid circular dependency
const { getProviderDefaults, clearProviderDefaultsCache } = require('./provider-defaults');

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
const LEVEL_RANKS = { level1: 1, level2: 2, level3: 3 };

/**
 * Validate a requested model against the maxModel ceiling and minModel floor
 * @param {string} requestedModel - Model the agent wants to use
 * @param {string} maxModel - Maximum allowed model (cost ceiling)
 * @param {string|null} minModel - Minimum required model (cost floor)
 * @returns {string} The validated model
 * @throws {Error} If requested model exceeds ceiling or falls below floor
 */
function validateModelAgainstMax(requestedModel, maxModel, minModel = null) {
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

  if (minModel) {
    if (!VALID_MODELS.includes(minModel)) {
      throw new Error(`Invalid minModel "${minModel}". Valid: ${VALID_MODELS.join(', ')}`);
    }
    if (MODEL_HIERARCHY[minModel] > MODEL_HIERARCHY[maxModel]) {
      throw new Error(`minModel "${minModel}" cannot be higher than maxModel "${maxModel}".`);
    }
    if (MODEL_HIERARCHY[requestedModel] < MODEL_HIERARCHY[minModel]) {
      throw new Error(
        `Agent requests "${requestedModel}" but minModel is "${minModel}". ` +
          `Either raise agent's model or lower minModel.`
      );
    }
  }

  return requestedModel;
}

// Default settings
const DEFAULT_SETTINGS = {
  maxModel: 'opus', // Cost ceiling - agents cannot use models above this
  minModel: null, // Cost floor - agents cannot use models below this (null = no minimum)
  defaultProvider: 'claude',
  get providerSettings() {
    // Dynamically build from providers on first access
    return getProviderDefaults();
  },
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
  // Valid presets: gh, git, ssh, aws, azure, kube, terraform, gcloud, claude, codex, gemini, opencode
  dockerMounts: ['gh', 'git', 'ssh'],
  // Extra env vars to pass to Docker container (in addition to preset-implied ones)
  // Supports: VAR (if set), VAR_* (pattern), VAR=value (forced), VAR= (empty)
  dockerEnvPassthrough: [],
  // Container home directory - where $HOME resolves in container paths
  // Default: /home/node (matches zeroshot-cluster-base image)
  dockerContainerHome: '/home/node',
};

function mapLegacyModelToLevel(model) {
  switch (model) {
    case 'haiku':
      return 'level1';
    case 'sonnet':
      return 'level2';
    case 'opus':
      return 'level3';
    default:
      return null;
  }
}

function mergeProviderSettings(current, overrides) {
  // Ensure current has all providers with their defaults
  const providerDefaults = getProviderDefaults();
  const merged = {};

  for (const provider of VALID_PROVIDERS) {
    merged[provider] = {
      ...(providerDefaults[provider] || {}),
      ...(current[provider] || {}),
      ...(overrides?.[provider] || {}),
    };
    if (!merged[provider].levelOverrides) {
      merged[provider].levelOverrides = {};
    }
  }
  return merged;
}

function applyLegacyModelBounds(settings) {
  if (!settings.providerSettings) return settings;
  const claude = settings.providerSettings.claude || {};
  const legacyMaxLevel = mapLegacyModelToLevel(settings.maxModel);
  const legacyMinLevel = mapLegacyModelToLevel(settings.minModel);

  if (legacyMaxLevel) {
    claude.maxLevel = legacyMaxLevel;
  }

  if (legacyMinLevel) {
    claude.minLevel = legacyMinLevel;
  }

  const minRank = LEVEL_RANKS[claude.minLevel] || LEVEL_RANKS.level1;
  const maxRank = LEVEL_RANKS[claude.maxLevel] || LEVEL_RANKS.level3;
  const defaultRank = LEVEL_RANKS[claude.defaultLevel] || LEVEL_RANKS.level2;

  if (minRank > maxRank) {
    claude.minLevel = 'level1';
    claude.maxLevel = 'level3';
  } else if (defaultRank < minRank) {
    claude.defaultLevel = claude.minLevel;
  } else if (defaultRank > maxRank) {
    claude.defaultLevel = claude.maxLevel;
  }

  settings.providerSettings.claude = claude;
  return settings;
}

function normalizeLoadedSettings(parsed) {
  const normalized = { ...parsed };
  if (parsed.defaultProvider) {
    normalized.defaultProvider = normalizeProviderName(parsed.defaultProvider);
  }
  if (parsed.providerSettings) {
    normalized.providerSettings = normalizeProviderSettings(parsed.providerSettings);
  }
  return normalized;
}

/**
 * Load settings from disk, merging with defaults
 */
function loadSettings() {
  const settingsFile = getSettingsFile();
  if (!fs.existsSync(settingsFile)) {
    // Return a copy with resolved providerSettings
    return {
      ...DEFAULT_SETTINGS,
      providerSettings: getProviderDefaults(),
    };
  }
  try {
    const data = fs.readFileSync(settingsFile, 'utf8');
    const parsed = normalizeLoadedSettings(JSON.parse(data));
    const merged = { ...DEFAULT_SETTINGS, ...parsed };
    merged.defaultProvider =
      normalizeProviderName(merged.defaultProvider) || DEFAULT_SETTINGS.defaultProvider;
    merged.providerSettings = mergeProviderSettings(getProviderDefaults(), parsed.providerSettings);
    return applyLegacyModelBounds(merged);
  } catch {
    console.error('Warning: Could not load settings, using defaults');
    return {
      ...DEFAULT_SETTINGS,
      providerSettings: getProviderDefaults(),
    };
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
 * Validate claudeCommand setting
 * @returns {string|null} Error message if invalid, null if valid
 */
function validateClaudeCommand(value) {
  if (typeof value !== 'string') {
    return 'claudeCommand must be a string';
  }
  if (value.trim().length === 0) {
    return 'claudeCommand cannot be empty';
  }
  return null;
}

/**
 * Validate providerSettings structure by delegating to provider implementations
 * @returns {string|null} Error message if invalid, null if valid
 */
function validateProviderSettings(value) {
  const normalizedSettings = normalizeProviderSettings(value);
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return 'providerSettings must be an object';
  }

  // Lazy require to avoid circular dependency
  const { getProvider } = require('../src/providers');

  for (const [providerName, settings] of Object.entries(normalizedSettings || {})) {
    if (!VALID_PROVIDERS.includes(providerName)) {
      return `Unknown provider in providerSettings: ${providerName}`;
    }

    // Delegate validation to the provider
    try {
      const provider = getProvider(providerName);
      const error = provider.validateSettings(settings);
      if (error) return error;
    } catch (err) {
      return `Failed to validate ${providerName} settings: ${err.message}`;
    }
  }

  return null;
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

  if (key === 'minModel' && value !== null && !VALID_MODELS.includes(value)) {
    return `Invalid model: ${value}. Valid models: ${VALID_MODELS.join(', ')}, null`;
  }

  if (key === 'logLevel' && !['quiet', 'normal', 'verbose'].includes(value)) {
    return `Invalid log level: ${value}. Valid levels: quiet, normal, verbose`;
  }

  if (key === 'claudeCommand') {
    return validateClaudeCommand(value);
  }

  if (key === 'defaultProvider') {
    const normalized = normalizeProviderName(value);
    if (!VALID_PROVIDERS.includes(normalized)) {
      return `Invalid provider: ${value}. Valid providers: ${VALID_PROVIDERS.join(', ')}`;
    }
  }

  if (key === 'providerSettings') {
    return validateProviderSettings(value);
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

  // Handle null values for minModel
  if (key === 'minModel' && (value === 'null' || value === null)) {
    return null;
  }

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

  if (key === 'providerSettings') {
    if (typeof value === 'string') {
      try {
        return normalizeProviderSettings(JSON.parse(value));
      } catch {
        throw new Error(`Invalid JSON for providerSettings: ${value}`);
      }
    }
    return normalizeProviderSettings(value);
  }

  if (key === 'defaultProvider') {
    return normalizeProviderName(value);
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
  // Provider defaults exports
  clearProviderDefaultsCache,
  // Backward compatibility: SETTINGS_FILE as getter (reads env var dynamically)
  get SETTINGS_FILE() {
    return getSettingsFile();
  },
};
