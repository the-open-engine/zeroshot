const BaseProvider = require('../base-provider');
const { getClaudeCommand, loadSettings } = require('../../../lib/settings');
const { commandExists, getCommandPath, getHelpOutput } = require('../../../lib/provider-detection');
const { resolveClaudeAuth } = require('../../../lib/settings/claude-auth');
const { buildCommand } = require('./cli-builder');
const { parseEvent } = require('./output-parser');
const {
  MODEL_CATALOG,
  LEVEL_MAPPING,
  DEFAULT_LEVEL,
  DEFAULT_MAX_LEVEL,
  DEFAULT_MIN_LEVEL,
} = require('./models');

const warned = new Set();

class AnthropicProvider extends BaseProvider {
  constructor() {
    super({ name: 'claude', displayName: 'Claude', cliCommand: 'claude' });
    this._cliFeatures = null;
  }

  // SDK not implemented - uses CLI only
  // See BaseProvider for SDK extension point documentation

  isAvailable() {
    const { command } = getClaudeCommand();
    return commandExists(command);
  }

  getCliPath() {
    const { command } = getClaudeCommand();
    return getCommandPath(command) || command;
  }

  getInstallInstructions() {
    return ['npm install -g @anthropic-ai/claude-code', 'Or (macOS): brew install claude'].join(
      '\n'
    );
  }

  getAuthInstructions() {
    return 'claude login';
  }

  getCliFeatures() {
    if (this._cliFeatures) return this._cliFeatures;

    const { command, args } = getClaudeCommand();
    const help = getHelpOutput(command, args);
    const unknown = !help;

    const features = {
      supportsOutputFormat: unknown ? true : /--output-format/.test(help),
      supportsStreamJson: unknown ? true : /stream-json/.test(help),
      supportsJsonSchema: unknown ? true : /--json-schema/.test(help),
      supportsAutoApprove: unknown ? true : /--dangerously-skip-permissions/.test(help),
      supportsIncludePartials: unknown ? true : /--include-partial-messages/.test(help),
      supportsVerbose: unknown ? true : /--verbose/.test(help),
      supportsModel: unknown ? true : /--model/.test(help),
      unknown,
    };

    this._cliFeatures = features;
    return features;
  }

  getCredentialPaths() {
    return ['~/.claude'];
  }

  /**
   * Resolve authentication environment variables for Claude CLI.
   * Handles Bedrock, API key, and OAuth authentication.
   * @returns {Object} Environment variables for authentication
   */
  resolveAuthEnv() {
    const settings = loadSettings();
    return resolveClaudeAuth(settings);
  }

  buildCommand(context, options) {
    const { command, args } = getClaudeCommand();
    const cliFeatures = options.cliFeatures || {};

    if (options.jsonSchema && options.outputFormat !== 'json' && !options.strictSchema) {
      this._warnOnce(
        'claude-jsonschema-stream',
        'jsonSchema requested with stream output; schema enforcement will be post-validated.'
      );
    }

    if (
      options.jsonSchema &&
      options.outputFormat === 'json' &&
      cliFeatures.supportsJsonSchema === false
    ) {
      this._warnOnce(
        'claude-jsonschema-flag',
        'Claude CLI does not support --json-schema; skipping schema flag.'
      );
    }

    if (options.autoApprove && cliFeatures.supportsAutoApprove === false) {
      this._warnOnce(
        'claude-auto-approve',
        'Claude CLI does not support --dangerously-skip-permissions; continuing without auto-approve.'
      );
    }

    const authEnv = this.resolveAuthEnv();
    const resolvedOptions = { ...options, cliFeatures, authEnv };

    return buildCommand(context, resolvedOptions, { command, args });
  }

  parseEvent(line) {
    return parseEvent(line);
  }

  getModelCatalog() {
    return MODEL_CATALOG;
  }

  getLevelMapping() {
    return LEVEL_MAPPING;
  }

  getDefaultLevel() {
    return DEFAULT_LEVEL;
  }

  getDefaultMaxLevel() {
    return DEFAULT_MAX_LEVEL;
  }

  getDefaultMinLevel() {
    return DEFAULT_MIN_LEVEL;
  }

  _warnOnce(key, message) {
    if (warned.has(key)) return;
    warned.add(key);
    console.warn(`⚠️ ${message}`);
  }

  /**
   * Get default settings including Claude-specific auth fields
   * @override
   */
  getDefaultSettings() {
    return {
      ...super.getDefaultSettings(),
      // Authentication (optional persistent storage)
      anthropicApiKey: null, // sk-ant-* key
      bedrockApiKey: null, // AWS_BEARER_TOKEN_BEDROCK value
      bedrockRegion: null, // AWS_REGION for Bedrock
    };
  }

  /**
   * Validate Claude-specific settings including auth fields
   * @override
   */
  validateSettings(settings) {
    // First validate base provider settings (levels, etc.)
    const baseError = super.validateSettings(settings);
    if (baseError) return baseError;

    // Claude-specific auth field validation
    const {
      isValidAnthropicKey,
      ANTHROPIC_KEY_PREFIX,
    } = require('../../../lib/settings/claude-auth');

    // Validate string-or-null fields
    for (const field of ['anthropicApiKey', 'bedrockApiKey', 'bedrockRegion']) {
      if (
        settings[field] !== undefined &&
        settings[field] !== null &&
        typeof settings[field] !== 'string'
      ) {
        return `providerSettings.claude.${field} must be a string or null`;
      }
    }

    // Additional prefix validation for Anthropic API key
    if (settings.anthropicApiKey && !isValidAnthropicKey(settings.anthropicApiKey)) {
      return `providerSettings.claude.anthropicApiKey must start with ${ANTHROPIC_KEY_PREFIX}`;
    }

    return null;
  }

  /**
   * Get Claude-specific setting field names
   * @override
   */
  getSettingsFields() {
    return [...super.getSettingsFields(), 'anthropicApiKey', 'bedrockApiKey', 'bedrockRegion'];
  }
}

module.exports = AnthropicProvider;
