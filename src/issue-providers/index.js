/**
 * Issue Provider Registry
 *
 * Manages issue provider registration and auto-detection
 */

const GitHubProvider = require('./github-provider');
const GitLabProvider = require('./gitlab-provider');
const JiraProvider = require('./jira-provider');
const AzureDevOpsProvider = require('./azure-devops-provider');
const { detectGitContext } = require('../../lib/git-remote-utils');

/** @type {Map<string, typeof IssueProvider>} */
const providers = new Map();

/**
 * Register a provider class
 * @param {typeof IssueProvider} ProviderClass
 */
function registerProvider(ProviderClass) {
  if (!ProviderClass.id) {
    throw new Error(`Provider ${ProviderClass.name} must define static id property`);
  }
  providers.set(ProviderClass.id, ProviderClass);
}

/**
 * Detect which provider matches the input
 * Automatically detects provider from git remote when in a git repository
 *
 * @param {string} input - User input (URL, issue key, number)
 * @param {Object} settings - User settings
 * @param {string|null} forceProvider - Force specific provider (from CLI flag)
 * @param {Object|null|undefined} [gitContextOverride] - Override git context (for testing)
 *   - undefined: auto-detect from git remote (default)
 *   - null: explicitly no git context
 *   - object: use provided git context
 * @returns {typeof IssueProvider|null}
 */
function detectProvider(input, settings, forceProvider = null, gitContextOverride = undefined) {
  // Force flag takes precedence
  if (forceProvider) {
    return providers.get(forceProvider) || null;
  }

  // Use provided gitContext or auto-detect
  const gitContext = gitContextOverride === undefined ? detectGitContext() : gitContextOverride;

  // Auto-detect by checking each provider's detectIdentifier
  for (const ProviderClass of providers.values()) {
    if (ProviderClass.detectIdentifier(input, settings, gitContext)) {
      return ProviderClass;
    }
  }

  return null;
}

/**
 * Get provider class by ID
 * @param {string} providerId - Provider identifier
 * @returns {typeof IssueProvider|null}
 */
function getProvider(providerId) {
  return providers.get(providerId) || null;
}

/**
 * List all registered provider IDs
 * @returns {string[]}
 */
function listProviders() {
  return Array.from(providers.keys());
}

/**
 * List providers that support PR/MR creation
 * @returns {string[]}
 */
function listPRProviders() {
  return Array.from(providers.values())
    .filter((p) => p.supportsPR())
    .map((p) => p.id);
}

/**
 * Determine which git platform to create PR/MR on.
 * ALWAYS uses git remote context, NEVER the issue provider.
 *
 * This is the unified replacement for platform-detector.js
 *
 * @param {string} [cwd=process.cwd()] - Working directory
 * @returns {string} Platform ID ('github', 'gitlab', 'azure-devops')
 * @throws {Error} If platform cannot be determined or doesn't support PRs
 *
 * @example
 * // In a GitHub repo
 * getPlatformForPR() // → 'github'
 *
 * @example
 * // In a GitLab repo with Jira issue
 * getPlatformForPR() // → 'gitlab' (creates GitLab MR, not related to Jira)
 */
function getPlatformForPR(cwd = process.cwd()) {
  const gitContext = detectGitContext(cwd);

  if (!gitContext?.provider) {
    throw new Error(
      'Cannot determine git platform for --pr mode. ' +
        'Ensure you are in a git repository with a remote URL from GitHub, GitLab, or Azure DevOps.'
    );
  }

  const platform = gitContext.provider;
  const ProviderClass = providers.get(platform);

  if (!ProviderClass || !ProviderClass.supportsPR()) {
    const supported = listPRProviders().join(', ');
    throw new Error(
      `Platform '${platform}' does not support --pr mode. Supported platforms: ${supported}`
    );
  }

  return platform;
}

/**
 * Get PR tool info for a platform
 * @param {string} platform - Platform ID
 * @returns {{ name: string, checkCmd: string, installHint: string, displayName: string }|null}
 */
function getPRToolForPlatform(platform) {
  const ProviderClass = providers.get(platform);
  return ProviderClass?.getPRTool() || null;
}

/**
 * Get aggregated settings schema from all issue providers
 * @returns {Object.<string, Object>} Combined settings schema
 */
function getIssueProviderSettingsSchema() {
  const schema = {};
  for (const ProviderClass of providers.values()) {
    Object.assign(schema, ProviderClass.getSettingsSchema());
  }
  return schema;
}

/**
 * Get default values for all issue provider settings
 * @returns {Object.<string, any>} Default values keyed by setting name
 */
function getIssueProviderSettingsDefaults() {
  const defaults = {};
  const schema = getIssueProviderSettingsSchema();
  for (const [key, config] of Object.entries(schema)) {
    defaults[key] = config.default;
  }
  return defaults;
}

/**
 * Validate an issue provider setting
 * Delegates to the appropriate provider's validateSetting method
 * @param {string} key - Setting key
 * @param {any} value - Setting value
 * @returns {string|null|undefined} Error message if invalid, null if valid, undefined if not an issue provider setting
 */
function validateIssueProviderSetting(key, value) {
  for (const ProviderClass of providers.values()) {
    const result = ProviderClass.validateSetting(key, value);
    if (result !== undefined) {
      return result;
    }
  }
  return undefined; // Not an issue provider setting
}

// Auto-register providers on module load
registerProvider(GitHubProvider);
registerProvider(GitLabProvider);
registerProvider(JiraProvider);
registerProvider(AzureDevOpsProvider);

module.exports = {
  registerProvider,
  detectProvider,
  getProvider,
  listProviders,
  listPRProviders,
  getPlatformForPR,
  getPRToolForPlatform,
  getIssueProviderSettingsSchema,
  getIssueProviderSettingsDefaults,
  validateIssueProviderSetting,
};
