/**
 * Linear Provider - Fetch issues from Linear via GraphQL API
 * Linear is not a git host; no CLI binary required (talks to api.linear.app directly)
 */

const IssueProvider = require('./base-provider');

const AUTH_CHECK_TIMEOUT_MS = 2000;
const FETCH_TIMEOUT_MS = 10000;
const LINEAR_API_URL = 'https://api.linear.app/graphql';
const LINEAR_URL_PATTERN = /linear\.app\/[^/]+\/issue\/([A-Z][A-Z0-9]*-\d+)/;
const LINEAR_KEY_PATTERN = /^[A-Z][A-Z0-9]*-\d+$/;

class LinearProvider extends IssueProvider {
  static id = 'linear';
  static displayName = 'Linear';

  /**
   * Detect Linear issue keys and URLs
   * Matches:
   * - Linear issue URLs (linear.app/<workspace>/issue/<TEAM>-<n>/...)
   * - Linear issue keys (TEAM-123 format)
   * - Bare numbers when defaultIssueSource=linear (requires linearTeam setting)
   *
   * Note: Linear doesn't use git context since it's not a git hosting platform.
   * Git context will never match 'linear', so this provider only activates via
   * explicit Linear URLs, Linear issue keys, or settings.
   *
   * Note: Linear and Jira share the same KEY-NUMBER key format (e.g. ENG-42 vs
   * PROJ-123), so a bare key is ambiguous between the two providers. Registration
   * order in index.js resolves this for auto-detection — Linear is registered
   * after Jira, so an ambiguous bare KEY-NUMBER key matches Jira first. Use
   * --linear or defaultIssueSource=linear for unambiguous Linear selection.
   *
   * @param {string} input - Issue identifier (URL, key, or number)
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Git context (unused for Linear, but kept for API consistency)
   * @returns {boolean} True if this provider should handle the input
   */
  static detectIdentifier(input, settings, gitContext = null) {
    // Linear issue URLs
    if (LINEAR_URL_PATTERN.test(input)) {
      return true;
    }

    // Linear issue key pattern (TEAM-123)
    if (LINEAR_KEY_PATTERN.test(input)) {
      return true;
    }

    // Bare numbers - use shared priority cascade logic
    // Linear requires linearTeam setting to convert bare numbers to issue keys
    // Note: gitContext check will never match 'linear' since Linear isn't a git host
    return IssueProvider.detectBareNumber(input, settings, gitContext, 'linear', {
      requiredSettings: ['linearTeam'],
    });
  }

  static getRequiredTool() {
    // No CLI binary required - talks to Linear's GraphQL API directly via fetch
    return {
      name: null,
      checkCmd: null,
      installHint: null,
    };
  }

  /**
   * Validate response status and parse GraphQL JSON body.
   * Shared by checkAuth and fetchIssue so both surface 401/429/non-OK/invalid-body
   * failures the same way instead of falling through to a generic JSON-parse error.
   * @private
   */
  static async _handleGraphQLResponse(response) {
    if (response.status === 401) {
      throw new Error('Linear API key invalid');
    }

    if (response.status === 429) {
      throw new Error('Linear API rate limit exceeded, try again later');
    }

    if (!response.ok) {
      throw new Error(`Linear API request failed with status ${response.status}`);
    }

    try {
      return await response.json();
    } catch {
      throw new Error(`Linear API returned an invalid response (status ${response.status})`);
    }
  }

  /**
   * Check Linear API key authentication via a minimal GraphQL query
   */
  static async checkAuth() {
    const apiKey = process.env.LINEAR_API_KEY;

    if (!apiKey) {
      return {
        authenticated: false,
        error: 'LINEAR_API_KEY not set',
        recovery: [
          'Create a personal API key at https://linear.app/settings/api',
          'export LINEAR_API_KEY=lin_api_...',
        ],
      };
    }

    try {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), AUTH_CHECK_TIMEOUT_MS);

      let response;
      try {
        response = await fetch(LINEAR_API_URL, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: apiKey,
          },
          body: JSON.stringify({ query: '{ viewer { id } }' }),
          signal: controller.signal,
        });
      } finally {
        clearTimeout(timeout);
      }

      const json = await LinearProvider._handleGraphQLResponse(response);
      if (json.errors) {
        return {
          authenticated: false,
          error: 'Linear API key invalid',
          recovery: ['Verify LINEAR_API_KEY at https://linear.app/settings/api'],
        };
      }

      return { authenticated: true, error: null, recovery: [] };
    } catch (err) {
      if (err.message === 'Linear API key invalid') {
        return {
          authenticated: false,
          error: err.message,
          recovery: ['Verify LINEAR_API_KEY at https://linear.app/settings/api'],
        };
      }

      return {
        authenticated: false,
        error: err.message,
        recovery: ['Check network connectivity', 'Verify https://api.linear.app is reachable'],
      };
    }
  }

  /**
   * Linear-specific settings schema
   */
  static getSettingsSchema() {
    return {
      linearTeam: {
        type: 'string',
        nullable: true,
        default: null,
        description: "Default Linear team key for bare numbers (e.g., 'ENG')",
        pattern: /^[A-Z][A-Z0-9]*$/,
        patternMsg: 'linearTeam must be a valid Linear team key (e.g., ENG, PROJ)',
      },
    };
  }

  async fetchIssue(identifier, settings) {
    const issueKey = this._extractIssueKey(identifier, settings);

    const apiKey = process.env.LINEAR_API_KEY;
    if (!apiKey) {
      throw new Error(
        'Failed to fetch Linear issue: LINEAR_API_KEY not set. ' +
          'Create a personal API key at https://linear.app/settings/api ' +
          'and export LINEAR_API_KEY=lin_api_...'
      );
    }

    const query = `
      query($id: String!) {
        issue(id: $id) {
          identifier
          number
          title
          description
          url
          # Linear defaults to a 50-node first page with no pagination here; long
          # label sets or comment threads beyond that are silently truncated.
          labels {
            nodes {
              name
            }
          }
          comments {
            nodes {
              user {
                name
              }
              createdAt
              body
            }
          }
        }
      }
    `;

    try {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS);

      let response;
      try {
        response = await fetch(LINEAR_API_URL, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Authorization: apiKey,
          },
          body: JSON.stringify({ query, variables: { id: issueKey } }),
          signal: controller.signal,
        });
      } finally {
        clearTimeout(timeout);
      }

      const json = await LinearProvider._handleGraphQLResponse(response);

      if (json.errors) {
        throw new Error(json.errors.map((e) => e.message).join('; '));
      }

      if (!json.data?.issue) {
        throw new Error(`Linear issue not found: ${issueKey}`);
      }

      return this._parseIssue(json.data.issue);
    } catch (error) {
      throw new Error(`Failed to fetch Linear issue: ${error.message}`);
    }
  }

  /**
   * Extract Linear issue key from URL or return as-is
   * @private
   */
  _extractIssueKey(identifier, settings) {
    // URL format: https://linear.app/<workspace>/issue/TEAM-123/...
    const urlMatch = identifier.match(LINEAR_URL_PATTERN);
    if (urlMatch) {
      return urlMatch[1];
    }

    // Bare number: construct from linearTeam setting
    if (/^\d+$/.test(identifier) && settings.linearTeam) {
      return `${settings.linearTeam}-${identifier}`;
    }

    // Assume it's already a key
    return identifier;
  }

  /**
   * Parse Linear issue into standardized InputData format
   * @private
   */
  _parseIssue(issue) {
    const labels = issue.labels?.nodes || [];
    const comments = issue.comments?.nodes || [];
    const description = issue.description || '';

    let context = `# Linear Issue ${issue.identifier}\n\n`;
    context += `## Title\n${issue.title}\n\n`;

    if (description) {
      context += `## Description\n${description}\n\n`;
    }

    if (labels.length > 0) {
      context += `## Labels\n`;
      context += labels.map((l) => `- ${l.name}`).join('\n');
      context += '\n\n';
    }

    if (comments.length > 0) {
      context += `## Comments\n\n`;
      for (const comment of comments) {
        const author = comment.user?.name || 'unknown';
        context += `### ${author} (${comment.createdAt})\n`;
        context += `${comment.body}\n\n`;
      }
    }

    const mappedLabels = labels.map((l) => ({ name: l.name }));
    const mappedComments = comments.map((c) => ({
      author: { login: c.user?.name || 'unknown' },
      createdAt: c.createdAt,
      body: c.body,
    }));

    return {
      number: issue.number,
      title: issue.title,
      body: description,
      labels: mappedLabels,
      comments: mappedComments,
      url: issue.url || null,
      context,
    };
  }
}

module.exports = LinearProvider;
