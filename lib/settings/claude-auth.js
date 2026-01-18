/**
 * Claude authentication settings and helpers
 * Extracted from settings.js for provider-specific isolation
 */

/**
 * Anthropic API key prefix for validation
 */
const ANTHROPIC_KEY_PREFIX = 'sk-ant-';

/**
 * Environment variables used for Claude authentication
 */
const CLAUDE_AUTH_ENV_VARS = [
  'ANTHROPIC_API_KEY',
  'AWS_BEARER_TOKEN_BEDROCK',
  'AWS_REGION',
  'CLAUDE_CODE_USE_BEDROCK',
];

/**
 * Validate an Anthropic API key format
 * @param {string|null|undefined} key - API key to validate
 * @returns {boolean} True if key is falsy OR starts with valid prefix
 */
function isValidAnthropicKey(key) {
  return !key || key.startsWith(ANTHROPIC_KEY_PREFIX);
}

/**
 * Check if Bedrock mode is active based on env and overrides
 * @param {Object} [envOverrides={}] - Environment variable overrides to check
 * @returns {boolean} True if Bedrock mode is active
 */
function isBedrockMode(envOverrides = {}) {
  return (
    envOverrides.CLAUDE_CODE_USE_BEDROCK === '1' || process.env.CLAUDE_CODE_USE_BEDROCK === '1'
  );
}

/**
 * Resolve Claude authentication environment variables from settings
 * Bedrock takes priority over direct Anthropic API key AND OAuth session
 * @param {Object} settings - Settings object from loadSettings()
 * @returns {Object} Environment variables to set for Claude auth
 */
function resolveClaudeAuth(settings) {
  const claudeSettings = settings.providerSettings?.claude || {};
  const env = {};

  // Bedrock takes priority over everything (including OAuth)
  if (!process.env.AWS_BEARER_TOKEN_BEDROCK && claudeSettings.bedrockApiKey) {
    env.AWS_BEARER_TOKEN_BEDROCK = claudeSettings.bedrockApiKey;
    env.CLAUDE_CODE_USE_BEDROCK = '1';
    if (claudeSettings.bedrockRegion && !process.env.AWS_REGION) {
      env.AWS_REGION = claudeSettings.bedrockRegion;
    }
  } else if (process.env.AWS_BEARER_TOKEN_BEDROCK && !process.env.CLAUDE_CODE_USE_BEDROCK) {
    // Auto-set CLAUDE_CODE_USE_BEDROCK if token present in env
    env.CLAUDE_CODE_USE_BEDROCK = '1';
  }

  // Anthropic API key only if no Bedrock
  const hasBedrock = env.AWS_BEARER_TOKEN_BEDROCK || process.env.AWS_BEARER_TOKEN_BEDROCK;
  if (!process.env.ANTHROPIC_API_KEY && !hasBedrock && claudeSettings.anthropicApiKey) {
    env.ANTHROPIC_API_KEY = claudeSettings.anthropicApiKey;
  }

  return env;
}

module.exports = {
  ANTHROPIC_KEY_PREFIX,
  CLAUDE_AUTH_ENV_VARS,
  isValidAnthropicKey,
  isBedrockMode,
  resolveClaudeAuth,
};
