const BaseProvider = require('../base-provider');
const { buildCommand } = require('./cli-builder');
const { parseEvent } = require('./output-parser');
const {
  MODEL_CATALOG,
  LEVEL_MAPPING,
  DEFAULT_LEVEL,
  DEFAULT_MAX_LEVEL,
  DEFAULT_MIN_LEVEL,
} = require('./models');

const MINIMAX_BASE_URL = 'https://api.minimax.io/v1';

const warned = new Set();

class MinimaxProvider extends BaseProvider {
  constructor() {
    super({ name: 'minimax', displayName: 'MiniMax', cliCommand: null });
  }

  getRetryableErrorPatterns() {
    return [
      ...super.getRetryableErrorPatterns(),
      /\brate[_ -]?limit\b/i,
      /\btoo many requests\b/i,
      /\bserver error\b/i,
    ];
  }

  getPermanentErrorPatterns() {
    return [
      ...super.getPermanentErrorPatterns(),
      /invalid[_ -]?api[_ -]?key/i,
      /\binvalid_request\b/i,
    ];
  }

  // ============================================================================
  // SDK SUPPORT - MiniMax is the first SDK-enabled provider
  // ============================================================================

  getSDKEnvVar() {
    return 'MINIMAX_API_KEY';
  }

  /**
   * Make a simple API call via the MiniMax OpenAI-compatible API.
   * @param {string} prompt - The prompt to send
   * @param {Object} options - Call options
   * @returns {Promise<{success: boolean, text: string, usage?: Object, error?: string}>}
   */
  async callSDK(prompt, options = {}) {
    const apiKey = process.env.MINIMAX_API_KEY;
    if (!apiKey) {
      return { success: false, error: 'MINIMAX_API_KEY environment variable is required' };
    }

    const model = options.model || 'MiniMax-M2.7';
    const maxTokens = options.maxTokens || 4096;

    const body = {
      model,
      messages: [{ role: 'user', content: prompt }],
      temperature: Math.max(0, Math.min(1, options.temperature ?? 0.01)),
      max_tokens: maxTokens,
      stream: false,
    };

    try {
      const response = await fetch(`${MINIMAX_BASE_URL}/chat/completions`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          Authorization: `Bearer ${apiKey}`,
        },
        body: JSON.stringify(body),
      });

      if (!response.ok) {
        const errorText = await response.text();
        return { success: false, error: `MiniMax API error (${response.status}): ${errorText}` };
      }

      const data = await response.json();
      const choice = data.choices?.[0];

      if (!choice) {
        return { success: false, error: 'No response from MiniMax API' };
      }

      let text = choice.message?.content || '';
      // Strip <think>...</think> tags from reasoning models
      text = text.replace(/<think>[\s\S]*?<\/think>/g, '').trim();

      return {
        success: true,
        text,
        usage: {
          inputTokens: data.usage?.prompt_tokens || 0,
          outputTokens: data.usage?.completion_tokens || 0,
        },
      };
    } catch (err) {
      return { success: false, error: `MiniMax API request failed: ${err.message}` };
    }
  }

  /**
   * Make a simple API call - uses SDK directly.
   * @param {string} prompt - The prompt to send
   * @param {Object} options - Call options (level, maxTokens, temperature)
   * @returns {Promise<{success: boolean, text: string, usage?: Object, error?: string}>}
   */
  async callSimple(prompt, options = {}) {
    const level = options.level || this.getDefaultLevel();
    const modelSpec = this.resolveModelSpec(level, {});
    return this.callSDK(prompt, {
      model: modelSpec.model,
      maxTokens: options.maxTokens,
      temperature: options.temperature,
    });
  }

  // ============================================================================
  // CLI SUPPORT - Uses embedded Node.js wrapper
  // ============================================================================

  isAvailable() {
    return !!process.env.MINIMAX_API_KEY;
  }

  getCliPath() {
    return `${process.execPath} (SDK-based, no external CLI)`;
  }

  getInstallInstructions() {
    return 'Set MINIMAX_API_KEY environment variable. Get your key at https://platform.minimaxi.com';
  }

  getAuthInstructions() {
    return 'export MINIMAX_API_KEY=your-api-key';
  }

  getCliFeatures() {
    return {
      supportsJson: true,
      supportsModel: true,
      supportsAutoApprove: false,
      supportsCwd: false,
      supportsVariant: false,
      unknown: false,
    };
  }

  getCredentialPaths() {
    return [];
  }

  buildCommand(context, options) {
    if (!process.env.MINIMAX_API_KEY) {
      this._warnOnce(
        'minimax-api-key',
        'MINIMAX_API_KEY not set. MiniMax provider requires an API key.'
      );
    }

    return buildCommand(context, options);
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

  getDefaultSettings() {
    return {
      ...super.getDefaultSettings(),
      minimaxApiKey: null,
    };
  }

  validateSettings(settings) {
    const baseError = super.validateSettings(settings);
    if (baseError) return baseError;

    if (
      settings.minimaxApiKey !== undefined &&
      settings.minimaxApiKey !== null &&
      typeof settings.minimaxApiKey !== 'string'
    ) {
      return 'providerSettings.minimax.minimaxApiKey must be a string or null';
    }

    return null;
  }

  getSettingsFields() {
    return [...super.getSettingsFields(), 'minimaxApiKey'];
  }

  _warnOnce(key, message) {
    if (warned.has(key)) return;
    warned.add(key);
    console.warn(`⚠️ ${message}`);
  }
}

module.exports = MinimaxProvider;
