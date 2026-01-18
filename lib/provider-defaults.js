/**
 * Provider defaults module
 *
 * Separated from settings.js to break circular dependency:
 * - settings.js requires provider defaults
 * - providers require settings.js (for loadSettings, getClaudeCommand, etc.)
 *
 * CRITICAL: This module should NOT import from settings.js
 */

// Cache provider defaults to avoid repeated instantiation
let _providerDefaultsCache = null;

/**
 * Build provider default settings by instantiating each provider
 * and calling getDefaultSettings()
 * @returns {Object} Map of provider name to default settings
 */
function buildProviderDefaults() {
  const { listProviders, getProvider } = require('../src/providers');

  const defaults = {};
  for (const providerName of listProviders()) {
    try {
      const provider = getProvider(providerName);
      defaults[providerName] = provider.getDefaultSettings();
    } catch (error) {
      // If provider fails to instantiate, use basic defaults
      console.warn(`Warning: Could not get defaults for ${providerName}: ${error.message}`);
      defaults[providerName] = {
        maxLevel: 'level3',
        minLevel: 'level1',
        defaultLevel: 'level2',
        levelOverrides: {},
      };
    }
  }
  return defaults;
}

/**
 * Get or build cached provider defaults
 * @returns {Object} Provider defaults
 */
function getProviderDefaults() {
  if (!_providerDefaultsCache) {
    _providerDefaultsCache = buildProviderDefaults();
  }
  return _providerDefaultsCache;
}

/**
 * Clear the provider defaults cache (primarily for testing)
 */
function clearProviderDefaultsCache() {
  _providerDefaultsCache = null;
}

module.exports = {
  getProviderDefaults,
  clearProviderDefaultsCache,
};
