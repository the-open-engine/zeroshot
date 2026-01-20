/**
 * Gitea Provider - Fetch issues from Gitea via tea CLI
 * Supports self-hosted Gitea instances only
 */

const IssueProvider = require('./base-provider');
const { execSync } = require('../lib/safe-exec');

class GiteaProvider extends IssueProvider {
  static id = 'gitea';
  static displayName = 'Gitea';

  /**
   * Gitea supports PR creation via tea CLI
   */
  static supportsPR() {
    return true;
  }

  /**
   * Get PR CLI tool info for preflight checks
   */
  static getPRTool() {
    return {
      name: 'tea',
      checkCmd: 'tea --version',
      installHint: 'brew install gitea/tap/tea (or https://gitea.com/gitea/tea)',
      displayName: 'Gitea',
    };
  }

  /**
   * Detect Gitea URLs and issue references
   * Matches:
   * - Gitea instance URLs (requires giteaInstance setting)
   * - org/repo#123 format when defaultIssueSource=gitea
   * - Bare numbers when defaultIssueSource=gitea
   *
   * @param {string} input - Issue identifier (URL, number, or org/repo#123)
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Auto-detected git remote context
   * @returns {boolean} True if this provider should handle the input
   */
  static detectIdentifier(input, settings, gitContext = null) {
    // Gitea instance URLs (requires giteaInstance setting)
    if (
      settings.giteaInstance &&
      input.includes(settings.giteaInstance) &&
      /\/issues\/\d+/.test(input)
    ) {
      return true;
    }

    // org/repo#123 format - use shared logic
    if (IssueProvider.detectRepoIssueFormat(input, settings, gitContext, 'gitea')) {
      return true;
    }

    // Bare numbers - use shared priority cascade logic
    // Note: Gitea is not in git-remote-utils, so this relies on defaultIssueSource setting
    return IssueProvider.detectBareNumber(input, settings, gitContext, 'gitea');
  }

  static getRequiredTool() {
    return {
      name: 'tea',
      checkCmd: 'tea --version',
      installHint: 'Install tea CLI: brew install gitea/tap/tea (or https://gitea.com/gitea/tea)',
    };
  }

  /**
   * Check tea CLI authentication.
   * Tea doesn't have a direct auth status command, so we use 'tea login ls' as a proxy.
   *
   * @param {string|null} hostname - Optional hostname for specific instance check
   */
  static checkAuth(hostname = null) {
    try {
      // tea login ls lists all configured logins
      // If this succeeds and returns logins, we're authenticated
      const output = execSync('tea login ls', { encoding: 'utf8', stdio: 'pipe' });

      // Check if the specific hostname is configured (if provided)
      if (hostname && !output.includes(hostname)) {
        return {
          authenticated: false,
          error: `tea CLI not authenticated for ${hostname}`,
          recovery: [
            `Run: tea login add --url https://${hostname}`,
            'Follow the authentication prompts',
            'Then verify: tea login ls',
          ],
        };
      }

      return { authenticated: true, error: null, recovery: [] };
    } catch {
      return {
        authenticated: false,
        error: `tea CLI not configured${hostname ? ` for ${hostname}` : ''}`,
        recovery: [
          hostname ? `Run: tea login add --url https://${hostname}` : 'Run: tea login add',
          'Follow the authentication prompts',
          'Then verify: tea login ls',
        ],
      };
    }
  }

  /**
   * Gitea-specific settings schema
   */
  static getSettingsSchema() {
    return {
      giteaInstance: {
        type: 'string',
        nullable: true,
        default: null,
        description: "Self-hosted Gitea URL (e.g., 'gitea.company.com')",
        pattern: /^[a-z0-9.-]+$/i,
        patternMsg: 'giteaInstance must be a valid hostname (e.g., gitea.company.com)',
      },
    };
  }

  fetchIssue(identifier, settings) {
    try {
      // Parse identifier to extract repo and issue number
      const { repo, issueNumber } = this._parseIdentifier(identifier, settings);

      // Tea CLI: Use issues list with specific issue filter
      // Note: 'tea issues <number>' shows detail but doesn't support --output json
      // So we use 'tea issues ls' with full fields and filter locally
      let cmd = `tea issues ls --output json --state all`;

      // Add all fields we need
      cmd += ` --fields index,title,state,author,body,created,updated,labels,url`;

      if (repo) {
        cmd += ` --repo ${repo}`;
      }

      // If we have a giteaInstance setting, we should specify which login to use
      const loginName = this._getLoginName(settings);
      if (loginName) {
        cmd += ` --login ${loginName}`;
      }

      const output = execSync(cmd, { encoding: 'utf8' });
      const issues = JSON.parse(output);

      // Find the specific issue by number
      const issue = issues.find((i) => i.index === issueNumber || i.index === String(issueNumber));

      if (!issue) {
        throw new Error(`Issue #${issueNumber} not found in repository${repo ? ` ${repo}` : ''}`);
      }

      // For comments, we need to make a separate call or parse from single issue view
      // Let's try to get comments via the single issue view
      let comments = [];
      try {
        let commentCmd = `tea issues ${issueNumber} --comments`;
        if (repo) {
          commentCmd += ` --repo ${repo}`;
        }
        if (loginName) {
          commentCmd += ` --login ${loginName}`;
        }

        const commentOutput = execSync(commentCmd, { encoding: 'utf8' });
        comments = this._parseComments(commentOutput);
      } catch {
        // Comments fetch failed, continue without them
      }

      return this._parseIssue(issue, comments);
    } catch (error) {
      throw new Error(`Failed to fetch Gitea issue: ${error.message}`);
    }
  }

  /**
   * Extract login name from tea configuration for the configured Gitea instance
   * @private
   */
  _getLoginName(settings) {
    if (!settings.giteaInstance) {
      return null;
    }

    try {
      // Get list of logins and find one matching the instance
      const output = execSync('tea login ls', { encoding: 'utf8', stdio: 'pipe' });
      const lines = output.split('\n');

      // Parse the table to find the login name for our instance
      for (const line of lines) {
        if (line.includes(settings.giteaInstance)) {
          // Extract the name from the first column
          const match = line.match(/│\s*(\S+)\s*│/);
          if (match) {
            return match[1];
          }
        }
      }
    } catch {
      // If we can't get logins, return null and let tea CLI use default
    }

    return null;
  }

  /**
   * Parse identifier to extract repo and issue number
   * @private
   */
  _parseIdentifier(identifier, _settings) {
    // URL format: https://gitea.example.com/org/repo/issues/123
    const urlMatch = identifier.match(/https?:\/\/([^/]+)\/([^/]+\/[^/]+)\/issues\/(\d+)/);
    if (urlMatch) {
      return {
        repo: urlMatch[2],
        issueNumber: urlMatch[3],
      };
    }

    // org/repo#123 format
    const repoMatch = identifier.match(/^([\w.-]+\/[\w.-]+)#(\d+)$/);
    if (repoMatch) {
      return { repo: repoMatch[1], issueNumber: repoMatch[2] };
    }

    // Bare number (no repo context - relies on git context or current directory)
    return { repo: null, issueNumber: identifier };
  }

  /**
   * Parse comments from tea issues output
   * @private
   */
  _parseComments(output) {
    const comments = [];

    // Look for the Comments section
    const commentsMatch = output.match(/## Comments\s+([\s\S]+)/);
    if (!commentsMatch) {
      return comments;
    }

    const commentsText = commentsMatch[1];

    // Parse individual comments: **@username** wrote on date:
    // Note: Date can contain colons (e.g., "2026-01-18 15:09")
    const commentPattern = /\*\*@([^*]+)\*\*\s+wrote on\s+(.+?):\s*\n+([\s\S]+?)(?=\n\s*\*\*@|$)/g;
    let match;

    while ((match = commentPattern.exec(commentsText)) !== null) {
      comments.push({
        author: { login: match[1].trim() },
        createdAt: match[2].trim(),
        body: match[3].trim(),
      });
    }

    return comments;
  }

  /**
   * Parse Gitea issue into standardized InputData format
   * @private
   */
  _parseIssue(issue, comments = []) {
    let context = `# Gitea Issue #${issue.index}\n\n`;
    context += `## Title\n${issue.title}\n\n`;

    if (issue.body) {
      context += `## Description\n${issue.body}\n\n`;
    }

    if (issue.labels && issue.labels.length > 0) {
      context += `## Labels\n`;
      // Labels might be comma-separated string or array
      const labelList = Array.isArray(issue.labels) ? issue.labels : issue.labels.split(',');
      context += labelList.map((l) => `- ${l.trim()}`).join('\n');
      context += '\n\n';
    }

    if (comments && comments.length > 0) {
      context += `## Comments\n\n`;
      for (const comment of comments) {
        context += `### ${comment.author.login} (${comment.createdAt})\n`;
        context += `${comment.body}\n\n`;
      }
    }

    // Map Gitea labels to standard format
    let labels = [];
    if (issue.labels) {
      const labelList = Array.isArray(issue.labels) ? issue.labels : issue.labels.split(',');
      labels = labelList.filter((l) => l && l.trim()).map((name) => ({ name: name.trim() }));
    }

    return {
      number: parseInt(issue.index, 10),
      title: issue.title,
      body: issue.body || '',
      labels,
      comments: comments || [],
      url: issue.url || null,
      context,
    };
  }
}

module.exports = GiteaProvider;
