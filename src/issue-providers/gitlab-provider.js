/**
 * GitLab Provider - Fetch issues from GitLab via glab CLI
 * Supports both cloud (gitlab.com) and self-hosted instances
 */

const IssueProvider = require('./base-provider');
const { execSync } = require('../lib/safe-exec');

const AUTH_CHECK_TIMEOUT_MS = 2000;

class GitLabProvider extends IssueProvider {
  static id = 'gitlab';
  static displayName = 'GitLab';

  /**
   * GitLab supports MR creation via glab CLI
   */
  static supportsPR() {
    return true;
  }

  /**
   * Get MR CLI tool info for preflight checks
   */
  static getPRTool() {
    return {
      name: 'glab',
      checkCmd: 'glab --version',
      installHint: 'brew install glab (or https://gitlab.com/gitlab-org/cli)',
      displayName: 'GitLab',
    };
  }

  /**
   * Detect GitLab URLs and issue references
   * Matches:
   * - gitlab.com URLs
   * - Self-hosted GitLab URLs (via gitlabInstance setting)
   * - org/repo#123 when git remote is GitLab (auto-detected)
   * - org/repo#123 when defaultIssueSource=gitlab
   * - Bare numbers when git remote is GitLab (auto-detected)
   * - Bare numbers when defaultIssueSource=gitlab
   *
   * @param {string} input - Issue identifier (URL, number, or org/repo#123)
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Auto-detected git remote context
   * @returns {boolean} True if this provider should handle the input
   */
  static detectIdentifier(input, settings, gitContext = null) {
    // GitLab cloud URLs
    if (input.includes('gitlab.com') && /\/-\/issues\/\d+/.test(input)) {
      return true;
    }

    // Self-hosted GitLab URLs
    if (
      settings.gitlabInstance &&
      input.includes(settings.gitlabInstance) &&
      /\/-\/issues\/\d+/.test(input)
    ) {
      return true;
    }

    // org/repo#123 format - use shared logic
    if (IssueProvider.detectRepoIssueFormat(input, settings, gitContext, 'gitlab')) {
      return true;
    }

    // Bare numbers - use shared priority cascade logic
    return IssueProvider.detectBareNumber(input, settings, gitContext, 'gitlab');
  }

  static getRequiredTool() {
    return {
      name: 'glab',
      checkCmd: 'glab --version',
      installHint: 'Install glab CLI: brew install glab (or https://gitlab.com/gitlab-org/cli)',
    };
  }

  /**
   * Check glab CLI authentication for a specific hostname.
   * If hostname is provided, only checks that specific instance.
   * Otherwise, uses glab's default context detection (git remote).
   *
   * @param {string|null} hostname - GitLab hostname to check (e.g., 'gitlab.com', 'gitlab.lrz.de')
   */
  static checkAuth(hostname = null) {
    // Use --hostname flag to check specific instance, avoiding false failures
    // when other configured instances are unauthenticated
    const cmd = hostname ? `glab auth status --hostname ${hostname}` : 'glab auth status';

    try {
      execSync(cmd, { encoding: 'utf8', stdio: 'pipe', timeout: AUTH_CHECK_TIMEOUT_MS });
      return { authenticated: true, error: null, recovery: [] };
    } catch (err) {
      const stderr = err.stderr || err.message || '';

      if (err.code === 'ENOENT' || stderr.includes('command not found')) {
        return {
          authenticated: false,
          error: 'glab CLI not installed',
          recovery: [
            'Install glab: https://gitlab.com/gitlab-org/cli',
            'Then verify: glab --version',
          ],
        };
      }

      if (stderr.includes('Command timed out')) {
        return {
          authenticated: false,
          error: 'glab auth status timed out',
          recovery: ['Retry: glab auth status', 'If it still hangs, run: glab auth login'],
        };
      }

      const hostnameHint = hostname || 'your GitLab instance';
      if (
        stderr.includes('not logged in') ||
        stderr.includes('no authentication') ||
        stderr.includes('No token found')
      ) {
        return {
          authenticated: false,
          error: `glab CLI not authenticated for ${hostnameHint}`,
          recovery: [
            hostname ? `Run: glab auth login --hostname ${hostname}` : 'Run: glab auth login',
            'Authenticate via browser when prompted',
            `Then verify: glab auth status${hostname ? ` --hostname ${hostname}` : ''}`,
          ],
        };
      }

      return {
        authenticated: false,
        error: stderr.trim() || 'Unknown glab auth error',
        recovery: [
          hostname ? `Run: glab auth login --hostname ${hostname}` : 'Run: glab auth login',
          'Then verify: glab auth status',
        ],
      };
    }
  }

  /**
   * GitLab-specific settings schema
   */
  static getSettingsSchema() {
    return {
      gitlabInstance: {
        type: 'string',
        nullable: true,
        default: null,
        description: "Self-hosted GitLab URL (e.g., 'gitlab.company.com')",
      },
    };
  }

  fetchIssue(identifier, _settings) {
    try {
      // glab accepts full URLs directly - use that for simplicity
      const isUrl = /^https?:\/\//.test(identifier);
      let cmd;

      if (isUrl) {
        // Pass URL directly to glab - handles self-hosted instances automatically
        cmd = `glab issue view "${identifier}" --output json`;
      } else {
        // For org/repo#123 or bare numbers, parse and build command
        const { issueNumber, repo } = this._parseIdentifier(identifier);
        cmd = `glab issue view ${issueNumber} --output json`;
        if (repo) {
          cmd += ` --repo ${repo}`;
        }
      }

      const output = execSync(cmd, { encoding: 'utf8' });
      const issue = JSON.parse(output);

      return this._parseIssue(issue);
    } catch (error) {
      throw new Error(`Failed to fetch GitLab issue: ${error.message}`);
    }
  }

  /**
   * Parse identifier to extract issue number, repo, and hostname
   * @private
   */
  _parseIdentifier(identifier) {
    // URL format: https://gitlab.example.com/org/repo/-/issues/123
    const urlMatch = identifier.match(/https?:\/\/([^/]+)\/([^/]+\/[^/]+)\/-\/issues\/(\d+)/);
    if (urlMatch) {
      const hostname = urlMatch[1];
      return {
        hostname: hostname !== 'gitlab.com' ? hostname : null,
        repo: urlMatch[2],
        issueNumber: urlMatch[3],
      };
    }

    // org/repo#123 format
    const repoMatch = identifier.match(/^([\w-]+\/[\w-]+)#(\d+)$/);
    if (repoMatch) {
      return { hostname: null, repo: repoMatch[1], issueNumber: repoMatch[2] };
    }

    // Bare number (relies on glab's default repo)
    return { hostname: null, repo: null, issueNumber: identifier };
  }

  /**
   * Parse GitLab issue into standardized InputData format
   * @private
   */
  _parseIssue(issue) {
    let context = `# GitLab Issue #${issue.iid || issue.id}\n\n`;
    context += `## Title\n${issue.title}\n\n`;

    if (issue.description) {
      context += `## Description\n${issue.description}\n\n`;
    }

    if (issue.labels && issue.labels.length > 0) {
      context += `## Labels\n`;
      context += issue.labels.map((l) => `- ${l}`).join('\n');
      context += '\n\n';
    }

    if (issue.notes && issue.notes.length > 0) {
      context += `## Comments\n\n`;
      for (const note of issue.notes) {
        const author = note.author?.username || 'unknown';
        context += `### ${author} (${note.created_at})\n`;
        context += `${note.body}\n\n`;
      }
    }

    // Map GitLab labels to GitHub format
    const labels = (issue.labels || []).map((name) => ({ name }));

    // Map GitLab notes to GitHub comment format
    const comments = (issue.notes || []).map((note) => ({
      author: { login: note.author?.username || 'unknown' },
      createdAt: note.created_at,
      body: note.body,
    }));

    return {
      number: issue.iid || issue.id || null,
      title: issue.title,
      body: issue.description || '',
      labels,
      comments,
      url: issue.web_url || null,
      context,
    };
  }
}

module.exports = GitLabProvider;
