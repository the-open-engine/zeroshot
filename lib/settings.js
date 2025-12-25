/**
 * Settings management for zeroshot
 * Persistent user preferences stored in ~/.zeroshot/settings.json
 */

const fs = require('fs');
const path = require('path');
const os = require('os');

// Settings file path
const SETTINGS_FILE = path.join(os.homedir(), '.zeroshot', 'settings.json');

// Default settings
const DEFAULT_SETTINGS = {
  defaultModel: 'sonnet',
  defaultConfig: 'conductor-bootstrap',
  defaultIsolation: false,
  strictSchema: false, // false = live streaming (default), true = guaranteed schema compliance (no streaming)
  logLevel: 'normal',
};

/**
 * Load settings from disk, merging with defaults
 */
function loadSettings() {
  if (!fs.existsSync(SETTINGS_FILE)) {
    return { ...DEFAULT_SETTINGS };
  }
  try {
    const data = fs.readFileSync(SETTINGS_FILE, 'utf8');
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
  const dir = path.dirname(SETTINGS_FILE);
  if (!fs.existsSync(dir)) {
    fs.mkdirSync(dir, { recursive: true });
  }
  fs.writeFileSync(SETTINGS_FILE, JSON.stringify(settings, null, 2), 'utf8');
}

/**
 * Validate a setting value
 * @returns {string|null} Error message if invalid, null if valid
 */
function validateSetting(key, value) {
  if (!(key in DEFAULT_SETTINGS)) {
    return `Unknown setting: ${key}`;
  }

  if (key === 'defaultModel' && !['opus', 'sonnet', 'haiku'].includes(value)) {
    return `Invalid model: ${value}. Valid models: opus, sonnet, haiku`;
  }

  if (key === 'logLevel' && !['quiet', 'normal', 'verbose'].includes(value)) {
    return `Invalid log level: ${value}. Valid levels: quiet, normal, verbose`;
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

  return value;
}

module.exports = {
  loadSettings,
  saveSettings,
  validateSetting,
  coerceValue,
  DEFAULT_SETTINGS,
  SETTINGS_FILE,
};
