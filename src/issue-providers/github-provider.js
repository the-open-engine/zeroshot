/**
 * GitHub Provider - Fetch issues from GitHub via gh CLI
 */

const IssueProvider = require('./base-provider');
const { execSync } = require('../lib/safe-exec');

const AUTH_CHECK_TIMEOUT_MS = 2000;

class GitHubProvider extends IssueProvider {
  static id = 'github';
  static displayName = 'GitHub';

  /**
   * GitHub supports PR creation via gh CLI
   */
  static supportsPR() {
    return true;
  }

  /**
   * Get PR CLI tool info for preflight checks
   */
  static getPRTool() {
    return {
      name: 'gh',
      checkCmd: 'gh --version',
      installHint: 'https://cli.github.com/',
      displayName: 'GitHub',
    };
  }

  /**
   * Detect GitHub URLs and issue references
   * Matches:
   * - github.com URLs
   * - org/repo#123 format
   * - Bare numbers when git remote is GitHub (auto-detected)
   * - Bare numbers when defaultIssueSource=github
   *
   * @param {string} input - Issue identifier (URL, number, or org/repo#123)
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Auto-detected git remote context
   * @returns {boolean} True if this provider should handle the input
   */
  static detectIdentifier(input, settings, gitContext = null) {
    // GitHub URLs
    if (input.includes('github.com') && /\/issues\/\d+/.test(input)) {
      return true;
    }

    // org/repo#123 format - use shared logic
    if (IssueProvider.detectRepoIssueFormat(input, settings, gitContext, 'github')) {
      return true;
    }

    // Bare numbers - use shared priority cascade logic
    return IssueProvider.detectBareNumber(input, settings, gitContext, 'github');
  }

  static getRequiredTool() {
    return {
      name: 'gh',
      checkCmd: 'gh --version',
      installHint: 'Install gh CLI: https://cli.github.com/',
    };
  }

  /**
   * Check gh CLI authentication
   */
  static checkAuth() {
    try {
      execSync('gh auth status', {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: AUTH_CHECK_TIMEOUT_MS,
      });
      return { authenticated: true, error: null, recovery: [] };
    } catch (err) {
      const stderr = err.stderr || err.message || '';

      if (err.code === 'ENOENT' || stderr.includes('command not found')) {
        return {
          authenticated: false,
          error: 'gh CLI not installed',
          recovery: ['Install gh: https://cli.github.com/', 'Then verify: gh --version'],
        };
      }

      if (stderr.includes('Command timed out')) {
        return {
          authenticated: false,
          error: 'gh auth status timed out',
          recovery: ['Retry: gh auth status', 'If it still hangs, re-run: gh auth login'],
        };
      }

      if (stderr.includes('not logged in')) {
        return {
          authenticated: false,
          error: 'gh CLI not authenticated',
          recovery: [
            'Run: gh auth login',
            'Select GitHub.com, HTTPS, and authenticate via browser',
            'Then verify: gh auth status',
          ],
        };
      }

      return {
        authenticated: false,
        error: stderr.trim() || 'Unknown gh auth error',
        recovery: ['Run: gh auth login', 'Then verify: gh auth status'],
      };
    }
  }

  /**
   * GitHub-specific settings schema
   * GitHub has no extra settings (cloud-only, no self-hosted config needed)
   */
  static getSettingsSchema() {
    return {};
  }

  fetchIssue(identifier, _settings, gitContext = null) {
    try {
      const { repo, number } = this._parseIdentifier(identifier, gitContext);

      // ALWAYS use -R flag when repo is known - never rely on CWD git remote
      const repoFlag = repo ? `-R ${repo}` : '';
      const cmd = `gh issue view ${number} ${repoFlag} --json number,title,body,labels,assignees,comments,url`;
      const output = execSync(cmd, { encoding: 'utf8' });
      const issue = JSON.parse(output);

      return this._parseIssue(issue);
    } catch (error) {
      throw new Error(`Failed to fetch GitHub issue: ${error.message}`);
    }
  }

  /**
   * Parse identifier into repo and issue number
   * @private
   * @returns {{ repo: string|null, number: string }}
   */
  _parseIdentifier(identifier, gitContext = null) {
    // GitHub URL: https://github.com/org/repo/issues/123
    const urlMatch = identifier.match(/github\.com\/([^/]+\/[^/]+)\/issues\/(\d+)/);
    if (urlMatch) {
      return { repo: urlMatch[1], number: urlMatch[2] };
    }

    // org/repo#123 format
    const repoMatch = identifier.match(/^([\w.-]+\/[\w.-]+)#(\d+)$/);
    if (repoMatch) {
      return { repo: repoMatch[1], number: repoMatch[2] };
    }

    // Bare number - use gitContext if available
    if (/^\d+$/.test(identifier)) {
      const repo =
        gitContext?.owner && gitContext?.repo ? `${gitContext.owner}/${gitContext.repo}` : null;
      return { repo, number: identifier };
    }

    // Fallback: assume it's a number, no repo
    return { repo: null, number: identifier };
  }

  /**
   * Parse issue into standardized InputData format
   * @private
   */
  _parseIssue(issue) {
    let context = `# GitHub Issue #${issue.number}\n\n`;
    context += `## Title\n${issue.title}\n\n`;

    if (issue.body) {
      context += `## Description\n${issue.body}\n\n`;
    }

    if (issue.labels && issue.labels.length > 0) {
      context += `## Labels\n`;
      context += issue.labels.map((l) => `- ${l.name}`).join('\n');
      context += '\n\n';
    }

    if (issue.comments && issue.comments.length > 0) {
      context += `## Comments\n\n`;
      for (const comment of issue.comments) {
        context += `### ${comment.author.login} (${new Date(comment.createdAt).toISOString()})\n`;
        context += `${comment.body}\n\n`;
      }
    }

    return {
      number: issue.number,
      title: issue.title,
      body: issue.body,
      labels: issue.labels || [],
      comments: issue.comments || [],
      url: issue.url || null,
      context,
    };
  }
}

module.exports = GitHubProvider;
