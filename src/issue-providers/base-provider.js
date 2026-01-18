/**
 * Base Provider - Abstract base class for issue providers
 *
 * This class represents a "platform" that can:
 * 1. Fetch issues (all providers)
 * 2. Create PRs/MRs (git-hosting providers only: GitHub, GitLab, Azure DevOps)
 *
 * Defines interface for fetching issues from different sources (GitHub, GitLab, Jira, etc)
 * Each provider must implement:
 * - Static properties: id, displayName
 * - Static methods: detectIdentifier(), getRequiredTool()
 * - Instance method: fetchIssue()
 *
 * Git-hosting providers should also implement:
 * - Static method: supportsPR() returning true
 * - Static method: getPRTool() for PR/MR CLI tool info
 */

class IssueProvider {
  /**
   * Provider identifier (e.g., 'github', 'gitlab')
   * @type {string}
   */
  static id = null;

  /**
   * Human-readable provider name
   * @type {string}
   */
  static displayName = null;

  /**
   * Whether this provider supports PR/MR creation.
   * Override in subclass to return true for git-hosting platforms.
   * @returns {boolean}
   */
  static supportsPR() {
    return false;
  }

  /**
   * Get PR/MR CLI tool info for preflight checks.
   * Only git-hosting providers (GitHub, GitLab, Azure DevOps) implement this.
   * @returns {{ name: string, checkCmd: string, installHint: string, displayName: string }|null}
   */
  static getPRTool() {
    return null;
  }

  /**
   * Detect if input matches this provider's patterns
   * @param {string} _input - User input (URL, issue key, number)
   * @param {Object} _settings - User settings from settings.js
   * @param {Object|null} _gitContext - Auto-detected git remote context
   * @returns {boolean}
   */
  static detectIdentifier(_input, _settings, _gitContext) {
    throw new Error(`${this.name}.detectIdentifier() must be implemented by subclass`);
  }

  /**
   * Shared detection logic for bare numbers (e.g., "123").
   * Implements the priority cascade:
   *   1. Git context (highest priority) - auto-detected from git remote
   *   2. Settings (defaultIssueSource)
   *   3. Legacy fallback (GitHub only, for backward compatibility)
   *
   * Call this from subclass detectIdentifier() for bare number handling.
   *
   * @param {string} input - User input to check
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Git context from detectGitContext()
   * @param {string} providerId - This provider's ID (e.g., 'github', 'gitlab')
   * @param {Object} [options] - Additional options
   * @param {boolean} [options.requiresAdditionalSettings] - If true, check additional settings exist
   * @param {string[]} [options.requiredSettings] - Settings keys that must exist for this provider
   * @returns {boolean} True if this provider should handle the bare number
   */
  static detectBareNumber(input, settings, gitContext, providerId, options = {}) {
    // Only handle bare numbers
    if (!/^\d+$/.test(input)) {
      return false;
    }

    // 1. Git context takes highest priority
    if (gitContext?.provider) {
      return gitContext.provider === providerId;
    }

    // 2. Check settings.defaultIssueSource
    if (settings.defaultIssueSource) {
      if (settings.defaultIssueSource !== providerId) {
        return false;
      }
      // If provider requires additional settings, validate them
      if (options.requiredSettings?.length > 0) {
        return options.requiredSettings.every((key) => settings[key]);
      }
      return true;
    }

    // 3. Legacy fallback - only GitHub gets the fallback for backward compatibility
    return providerId === 'github';
  }

  /**
   * Shared detection logic for org/repo#123 format.
   * Some providers (GitHub, GitLab) use this format.
   *
   * @param {string} input - User input to check
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Git context
   * @param {string} providerId - This provider's ID
   * @returns {boolean} True if this provider should handle the input
   */
  static detectRepoIssueFormat(input, settings, gitContext, providerId) {
    // Match org/repo#123 format
    if (!/^[\w.-]+\/[\w.-]+#\d+$/.test(input)) {
      return false;
    }

    // 1. Git context takes priority
    if (gitContext?.provider) {
      return gitContext.provider === providerId;
    }

    // 2. Settings fallback
    return settings.defaultIssueSource === providerId;
  }

  /**
   * Get required CLI tool info
   * @returns {{ name: string, checkCmd: string, installHint: string }}
   */
  static getRequiredTool() {
    throw new Error(`${this.name}.getRequiredTool() must be implemented by subclass`);
  }

  /**
   * Check if the provider's CLI is authenticated.
   * Override in subclass to implement provider-specific auth checks.
   *
   * @param {string|null} hostname - Optional hostname for providers with multi-instance support
   *   (e.g., GitLab with gitlab.com + self-hosted). When provided, only check auth for that
   *   specific instance to avoid false failures from other unconfigured instances.
   * @returns {{ authenticated: boolean, error: string|null, recovery: string[] }}
   *   - authenticated: true if auth is valid
   *   - error: error message if not authenticated, null otherwise
   *   - recovery: array of steps to fix auth issues
   */
  static checkAuth(_hostname = null) {
    // Default: no auth check required (some CLIs like jira use config files)
    return { authenticated: true, error: null, recovery: [] };
  }

  /**
   * Get settings schema for this provider.
   * Override in subclass to define provider-specific settings.
   *
   * Schema format:
   * {
   *   settingKey: {
   *     type: 'string' | 'number' | 'boolean',
   *     nullable: true | false,
   *     default: <default value>,
   *     description: 'Human-readable description',
   *     pattern?: RegExp,         // Optional validation pattern
   *     patternMsg?: string       // Error message if pattern fails
   *   }
   * }
   *
   * @returns {Object.<string, Object>} Settings schema
   */
  static getSettingsSchema() {
    return {};
  }

  /**
   * Validate a setting value for this provider.
   * Uses schema from getSettingsSchema() by default.
   *
   * @param {string} key - Setting key
   * @param {any} value - Setting value
   * @returns {string|null|undefined} Error message if invalid, null if valid, undefined if not this provider's setting
   */
  static validateSetting(key, value) {
    const schema = this.getSettingsSchema();
    const config = schema[key];

    if (!config) {
      return undefined; // Not this provider's setting
    }

    // Nullable check
    if (config.nullable && value === null) {
      return null; // null is valid
    }

    // Type validation
    if (config.type === 'string' && typeof value !== 'string') {
      return `${key} must be a string${config.nullable ? ' or null' : ''}`;
    }

    // Pattern validation
    if (config.pattern && typeof value === 'string' && !config.pattern.test(value)) {
      return config.patternMsg || `${key} has invalid format`;
    }

    return null; // Valid
  }

  /**
   * Fetch issue from provider
   * @param {string} _identifier - Issue identifier (URL, key, number)
   * @param {Object} _settings - User settings from settings.js
   * @returns {Promise<Object>} InputData object with structure:
   *   {
   *     number: Number|null,
   *     title: String,
   *     body: String,
   *     labels: Array<{name: String}>,
   *     comments: Array<{author, createdAt, body}>,
   *     url: String|null,
   *     context: String  // Formatted markdown
   *   }
   */
  fetchIssue(_identifier, _settings) {
    throw new Error(`${this.constructor.name}.fetchIssue() must be implemented by subclass`);
  }
}

module.exports = IssueProvider;
