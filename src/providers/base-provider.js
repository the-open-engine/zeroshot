/**
 * BaseProvider - Abstract provider interface
 *
 * All providers support two execution paths:
 * 1. SDK (fast) - Direct API calls using provider's SDK (requires API key)
 * 2. CLI (slower) - Spawns provider's CLI tool (uses OAuth/login auth)
 *
 * Use callSimple() for simple prompts - it tries SDK first, falls back to CLI.
 */
class BaseProvider {
  constructor(options = {}) {
    this.name = options.name || 'base';
    this.displayName = options.displayName || 'Base';
    this.cliCommand = options.cliCommand || null;
  }

  // ============================================================================
  // SDK SUPPORT (Future Extension Point)
  // ============================================================================
  //
  // SDK support is NOT IMPLEMENTED. These methods exist for future extension.
  // Currently, all providers use CLI (claude, codex, gemini) for execution.
  //
  // To add SDK support for a provider:
  // 1. Override getSDKEnvVar() to return the API key env var
  // 2. Override callSDK() to implement the actual API call
  // 3. The callSimple() method will then work automatically
  //
  // ============================================================================

  /**
   * Get the environment variable name for the API key
   * @returns {string} Environment variable name (e.g., 'ANTHROPIC_API_KEY')
   */
  getSDKEnvVar() {
    throw new Error(`${this.name}: SDK not implemented. Use CLI instead.`);
  }

  /**
   * Check if SDK is configured (API key is set)
   * @returns {boolean} True if API key is available
   */
  isSDKConfigured() {
    try {
      const envVar = this.getSDKEnvVar();
      return !!process.env[envVar];
    } catch {
      // getSDKEnvVar() throws if not implemented
      return false;
    }
  }

  /**
   * Make a simple API call via SDK (fast path)
   * NOT IMPLEMENTED - exists for future extension.
   *
   * @param {string} _prompt - The prompt to send
   * @param {Object} _options - Call options
   * @returns {Promise<{success: boolean, text: string, usage?: Object, error?: string}>}
   */
  callSDK(_prompt, _options) {
    return Promise.reject(new Error(`${this.name}: SDK not implemented. Use CLI instead.`));
  }

  /**
   * Make a simple API call via SDK
   * NOT IMPLEMENTED - exists for future extension.
   *
   * @param {string} _prompt - The prompt to send
   * @param {Object} _options - Call options
   * @returns {Promise<{success: boolean, text: string, usage?: Object, error?: string}>}
   */
  callSimple(_prompt, _options = {}) {
    return Promise.reject(new Error(`${this.name}: SDK not implemented. Use CLI instead.`));
  }

  isAvailable() {
    throw new Error('Not implemented');
  }

  getCliPath() {
    throw new Error('Not implemented');
  }

  getInstallInstructions() {
    throw new Error('Not implemented');
  }

  getAuthInstructions() {
    throw new Error('Not implemented');
  }

  getCliFeatures() {
    throw new Error('Not implemented');
  }

  getCredentialPaths() {
    return [];
  }

  buildCommand(_context, _options) {
    throw new Error('Not implemented');
  }

  parseEvent(_line) {
    throw new Error('Not implemented');
  }

  getModelCatalog() {
    throw new Error('Not implemented');
  }

  getLevelMapping() {
    throw new Error('Not implemented');
  }

  resolveModelSpec(level, overrides = {}) {
    const mapping = this.getLevelMapping();
    const base = mapping[level] || mapping[this.getDefaultLevel()];
    if (!base) {
      throw new Error(`Unknown level "${level}" for provider "${this.name}"`);
    }
    const override = overrides[level] || {};
    return {
      level,
      model: override.model || base.model,
      reasoningEffort: override.reasoningEffort || base.reasoningEffort,
    };
  }

  validateLevel(level, minLevel, maxLevel) {
    const mapping = this.getLevelMapping();
    const rank = (key) => mapping[key]?.rank;

    if (!mapping[level]) {
      throw new Error(`Invalid level "${level}" for provider "${this.name}"`);
    }

    if (minLevel && !mapping[minLevel]) {
      throw new Error(`Invalid minLevel "${minLevel}" for provider "${this.name}"`);
    }

    if (maxLevel && !mapping[maxLevel]) {
      throw new Error(`Invalid maxLevel "${maxLevel}" for provider "${this.name}"`);
    }

    if (minLevel && maxLevel && rank(minLevel) > rank(maxLevel)) {
      throw new Error(
        `minLevel "${minLevel}" exceeds maxLevel "${maxLevel}" for provider "${this.name}"`
      );
    }

    if (maxLevel && rank(level) > rank(maxLevel)) {
      throw new Error(
        `Level "${level}" exceeds maxLevel "${maxLevel}" for provider "${this.name}"`
      );
    }

    if (minLevel && rank(level) < rank(minLevel)) {
      throw new Error(
        `Level "${level}" is below minLevel "${minLevel}" for provider "${this.name}"`
      );
    }

    return level;
  }

  validateModelId(modelId) {
    const catalog = this.getModelCatalog();
    if (modelId && !catalog[modelId]) {
      throw new Error(`Invalid model "${modelId}" for provider "${this.name}"`);
    }
    return modelId;
  }

  /**
   * Resolve a model name to its CLI-compatible identifier.
   * Override in provider implementations that need model ID transformation
   * (e.g., Anthropic Bedrock mapping).
   * @param {string} model - Model name (e.g., 'opus', 'sonnet')
   * @param {Object} _authEnv - Authentication environment variables
   * @returns {string} CLI-compatible model identifier
   */
  resolveCliModel(model, _authEnv = {}) {
    return model;
  }

  getDefaultLevel() {
    throw new Error('Not implemented');
  }

  /**
   * Get default settings for this provider
   * Override in provider implementations to provide provider-specific defaults
   * @returns {Object} Default settings object
   */
  getDefaultSettings() {
    return {
      maxLevel: this.getDefaultMaxLevel?.() || 'level3',
      minLevel: this.getDefaultMinLevel?.() || 'level1',
      defaultLevel: this.getDefaultLevel() || 'level2',
      levelOverrides: {},
    };
  }

  /**
   * Validate provider-specific settings
   * Override in provider implementations to add custom validation
   * @param {Object} settings - Settings object to validate
   * @returns {string|null} Error message if invalid, null if valid
   */
  validateSettings(settings) {
    if (typeof settings !== 'object' || settings === null) {
      return `providerSettings.${this.name} must be an object`;
    }

    // Validate level fields
    const levelMapping = this.getLevelMapping();
    const validLevels = Object.keys(levelMapping);

    if (settings.maxLevel && !validLevels.includes(settings.maxLevel)) {
      return `Invalid maxLevel for ${this.name}: ${settings.maxLevel}`;
    }
    if (settings.minLevel && !validLevels.includes(settings.minLevel)) {
      return `Invalid minLevel for ${this.name}: ${settings.minLevel}`;
    }
    if (settings.defaultLevel && !validLevels.includes(settings.defaultLevel)) {
      return `Invalid defaultLevel for ${this.name}: ${settings.defaultLevel}`;
    }

    if (
      settings.levelOverrides &&
      (typeof settings.levelOverrides !== 'object' || Array.isArray(settings.levelOverrides))
    ) {
      return `levelOverrides for ${this.name} must be an object`;
    }

    return null;
  }

  /**
   * Get the list of setting field names specific to this provider
   * Override in provider implementations to declare custom fields
   * @returns {string[]} Array of field names
   */
  getSettingsFields() {
    return ['maxLevel', 'minLevel', 'defaultLevel', 'levelOverrides'];
  }
}

module.exports = BaseProvider;
