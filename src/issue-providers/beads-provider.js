/**
 * Beads Provider - Fetch issues from Beads (bd) CLI
 *
 * Beads is a git-backed issue tracker with dependency support.
 * https://github.com/steveyegge/beads
 *
 * Supports:
 * - Beads issue IDs: bd-abc123, AppKiln-xyz, project-123
 * - Auto-pick next issue: "beads:ready" or "beads:ready:P0" (priority filter)
 *
 * Lifecycle hooks:
 * - On success: closes issue, adds comment with PR URL
 * - On failure: adds comment with failure reason
 */

const IssueProvider = require('./base-provider');
const { execSync } = require('../lib/safe-exec');

class BeadsProvider extends IssueProvider {
  static id = 'beads';
  static displayName = 'Beads';

  /**
   * Beads doesn't support PR creation directly (it's not a git host)
   * PRs are created via the git host (GitHub, GitLab, etc.)
   */
  static supportsPR() {
    return false;
  }

  /**
   * Detect Beads issue identifiers
   * Matches:
   * - Standard beads IDs: bd-abc123, bd-a1b2c3d4
   * - Project-prefixed IDs: AppKiln-abc, MyProject-xyz123 (PascalCase prefix)
   * - Explicit beads: prefix: beads:bd-abc123, beads:AppKiln-xyz
   * - Auto-pick: beads:ready, beads:ready:P0, beads:ready:P1
   *
   * Does NOT match:
   * - Bare numbers (those go to GitHub/GitLab)
   * - lowercase-hyphen-words like fix-typo, my-branch (likely git branches)
   *
   * @param {string} input - Issue identifier
   * @param {Object} settings - User settings
   * @param {Object|null} _gitContext - Git context (unused for beads)
   * @returns {boolean} True if this looks like a Beads issue
   */
  static detectIdentifier(input, settings, _gitContext = null) {
    // Explicit beads: prefix - always match
    if (input.startsWith('beads:')) {
      return true;
    }

    // Standard beads ID format: bd-<hash>
    if (/^bd-[a-z0-9]+$/i.test(input)) {
      return true;
    }

    // Project-prefixed format: <PascalCaseProject>-<hash>
    // Requires: Capital letter start, then letters/numbers, hyphen, then alphanumeric hash
    // Examples: AppKiln-836, MyProject-abc123, JIRA-123
    // This distinguishes from branch names like: fix-typo, my-feature, add-tests
    if (/^[A-Z][a-z0-9]*-[a-z0-9]+$/i.test(input)) {
      // Check if first part is PascalCase or ALLCAPS (not lowercase)
      const prefix = input.split('-')[0];
      if (prefix[0] === prefix[0].toUpperCase()) {
        // Looks like a project prefix, check if beads can find it
        // This runs in current directory, so it works when run from the project
        try {
          execSync(`bd show ${input} --json`, {
            encoding: 'utf8',
            stdio: 'pipe',
            timeout: 5000,
          });
          return true;
        } catch {
          // bd couldn't find it - could be wrong directory or not a beads issue
          // If defaultIssueSource is beads, still claim it
          if (settings.defaultIssueSource === 'beads') {
            return true;
          }
          return false;
        }
      }
    }

    return false;
  }

  static getRequiredTool() {
    return {
      name: 'bd',
      checkCmd: 'bd --version',
      installHint:
        'Install Beads: go install github.com/steveyegge/beads/cmd/bd@latest\nSee: https://github.com/steveyegge/beads',
    };
  }

  /**
   * Check bd CLI availability
   * Beads doesn't require authentication (it's local git-backed)
   */
  static checkAuth() {
    try {
      execSync('bd --version', { encoding: 'utf8', stdio: 'pipe', timeout: 5000 });
      return { authenticated: true, error: null, recovery: [] };
    } catch {
      return {
        authenticated: false,
        error: 'bd CLI not found',
        recovery: [
          'Install Beads: go install github.com/steveyegge/beads/cmd/bd@latest',
          'Or see: https://github.com/steveyegge/beads',
          'Then verify: bd --version',
        ],
      };
    }
  }

  /**
   * Beads-specific settings schema
   * Note: beadsUpdateStatus removed - we always mark in_progress when starting
   */
  static getSettingsSchema() {
    return {};
  }

  /**
   * Fetch issue from Beads
   * Supports:
   * - Direct ID: AppKiln-836, bd-abc123
   * - Auto-pick: beads:ready, beads:ready:P0, beads:ready:P1
   *
   * @param {string} identifier - Beads issue ID or ready selector
   * @param {Object} _settings - User settings (unused currently)
   * @returns {Object} InputData object
   */
  fetchIssue(identifier, _settings) {
    try {
      const issueId = this._resolveIssueId(identifier);

      // Fetch issue using bd CLI
      const cmd = `bd show ${issueId} --json`;
      const output = execSync(cmd, { encoding: 'utf8', timeout: 10000 });
      const issues = JSON.parse(output);

      if (!issues || issues.length === 0) {
        throw new Error(`Issue ${issueId} not found`);
      }

      // bd show returns an array even for single issue
      const issue = issues[0];

      // Always mark as in_progress when starting work
      try {
        execSync(`bd update ${issueId} --status=in_progress`, {
          encoding: 'utf8',
          stdio: 'pipe',
          timeout: 5000,
        });
      } catch {
        // Non-fatal: issue might already be in_progress
      }

      return this._parseIssue(issue);
    } catch (error) {
      throw new Error(`Failed to fetch Beads issue: ${error.message}`);
    }
  }

  /**
   * Lifecycle hook: Called when cluster completes successfully
   * - Closes the issue
   * - Adds comment with PR URL if available
   */
  async onClusterComplete(inputData, result, _settings) {
    const issueId = inputData.beadsId;
    if (!issueId) return;

    // Use Promise.resolve to satisfy async requirement while using sync execSync
    await Promise.resolve();

    // Add comment with result
    const comment = result.prUrl
      ? `✅ ZeroShot completed successfully.\n\nPR: ${result.prUrl}\nCluster: ${result.clusterId}`
      : `✅ ZeroShot completed successfully.\n\nCluster: ${result.clusterId}`;

    try {
      execSync(`bd comments add ${issueId} --body "${this._escapeShellArg(comment)}"`, {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: 5000,
      });
    } catch {
      // Non-fatal: comment add might not be supported or fail
    }

    // Close the issue
    try {
      const closeReason = result.prUrl
        ? `Completed by ZeroShot. PR: ${result.prUrl}`
        : `Completed by ZeroShot (cluster: ${result.clusterId})`;
      execSync(`bd close ${issueId} --reason "${this._escapeShellArg(closeReason)}"`, {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: 5000,
      });
    } catch (err) {
      // Log but don't fail
      console.error(`[BeadsProvider] Failed to close issue ${issueId}: ${err.message}`);
    }
  }

  /**
   * Lifecycle hook: Called when cluster fails
   * - Adds comment with failure reason
   * - Does NOT close the issue (leaves it for retry)
   */
  async onClusterFailed(inputData, result, _settings) {
    const issueId = inputData.beadsId;
    if (!issueId) return;

    // Use Promise.resolve to satisfy async requirement while using sync execSync
    await Promise.resolve();

    // Add comment with failure info
    const comment = `❌ ZeroShot failed.\n\nReason: ${result.reason}\nCluster: ${result.clusterId}${result.error ? `\n\nError: ${result.error}` : ''}`;

    try {
      execSync(`bd comments add ${issueId} --body "${this._escapeShellArg(comment)}"`, {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: 5000,
      });
    } catch {
      // Non-fatal
    }

    // Update status back to open (so it shows up in bd ready again)
    try {
      execSync(`bd update ${issueId} --status=open`, {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: 5000,
      });
    } catch {
      // Non-fatal
    }
  }

  /**
   * Resolve issue ID from input
   * Handles:
   * - beads:ready - auto-pick highest priority ready issue
   * - beads:ready:P0 - auto-pick from specific priority
   * - beads:issue-id - explicit issue
   * - issue-id - direct ID
   * @private
   */
  _resolveIssueId(input) {
    // Remove beads: prefix if present
    if (input.startsWith('beads:')) {
      const rest = input.slice(6);

      // Check for ready selector
      if (rest === 'ready' || rest.startsWith('ready:')) {
        return this._pickReadyIssue(rest);
      }

      return rest;
    }
    return input;
  }

  /**
   * Pick the next ready issue using bd ready
   * @param {string} selector - 'ready' or 'ready:P0', 'ready:P1', etc.
   * @private
   */
  _pickReadyIssue(selector) {
    let cmd = 'bd ready --limit 1 --json';

    // Parse priority filter: ready:P0, ready:P1, etc.
    const priorityMatch = selector.match(/^ready:P(\d)$/i);
    if (priorityMatch) {
      const priority = priorityMatch[1];
      cmd = `bd ready --limit 1 --priority ${priority} --json`;
    }

    try {
      const output = execSync(cmd, { encoding: 'utf8', timeout: 10000 });
      const issues = JSON.parse(output);

      if (!issues || issues.length === 0) {
        throw new Error('No ready issues found. Use "bd ready" to check available work.');
      }

      const issue = issues[0];
      console.log(`[BeadsProvider] Auto-picked issue: ${issue.id} - ${issue.title}`);
      return issue.id;
    } catch (error) {
      if (error.message.includes('No ready issues')) {
        throw error;
      }
      throw new Error(`Failed to pick ready issue: ${error.message}`);
    }
  }

  /**
   * Escape string for shell argument
   * @private
   */
  _escapeShellArg(str) {
    return str.replace(/"/g, '\\"').replace(/\n/g, '\\n');
  }

  /**
   * Parse Beads issue into standardized InputData format
   * @private
   */
  _parseIssue(issue) {
    let context = `# Beads Issue ${issue.id}\n\n`;
    context += `## Title\n${issue.title}\n\n`;

    if (issue.description) {
      context += `## Description\n${issue.description}\n\n`;
    }

    // Status and priority
    context += `## Metadata\n`;
    context += `- **Status:** ${issue.status}\n`;
    context += `- **Priority:** ${issue.priority}\n`;
    if (issue.issue_type) {
      context += `- **Type:** ${issue.issue_type}\n`;
    }
    if (issue.owner) {
      context += `- **Owner:** ${issue.owner}\n`;
    }
    context += '\n';

    // Dependencies (blockers)
    if (issue.dependencies && issue.dependencies.length > 0) {
      context += `## Dependencies (Blocked By)\n`;
      context += `These issues must be completed before this one:\n\n`;
      for (const dep of issue.dependencies) {
        const status = dep.status === 'closed' ? '✅' : '⏳';
        context += `- ${status} **${dep.id}**: ${dep.title} (${dep.status})\n`;
      }
      context += '\n';

      // Warn if there are open blockers
      const openBlockers = issue.dependencies.filter((d) => d.status !== 'closed');
      if (openBlockers.length > 0) {
        context += `> ⚠️ **Warning:** ${openBlockers.length} blocking issue(s) still open.\n`;
        context += `> Consider working on blockers first or verify they're not actually required.\n\n`;
      }
    }

    // Labels (if any)
    if (issue.labels && issue.labels.length > 0) {
      context += `## Labels\n`;
      context += issue.labels.map((l) => `- ${l}`).join('\n');
      context += '\n\n';
    }

    // Extract acceptance criteria from description if present
    const acceptanceCriteria = this._extractAcceptanceCriteria(issue.description);
    if (acceptanceCriteria) {
      context += `## Acceptance Criteria (Extracted)\n`;
      context += acceptanceCriteria;
      context += '\n\n';
    }

    return {
      number: null, // Beads uses string IDs, not numbers
      title: issue.title,
      body: issue.description,
      labels: issue.labels ? issue.labels.map((l) => ({ name: l })) : [],
      comments: [], // Beads comments would need separate fetch
      url: null, // Beads is local, no web URL
      context,
      // Beads-specific metadata
      beadsId: issue.id,
      beadsStatus: issue.status,
      beadsPriority: issue.priority,
      beadsType: issue.issue_type,
      beadsDependencies: issue.dependencies || [],
    };
  }

  /**
   * Extract acceptance criteria section from description
   * @private
   */
  _extractAcceptanceCriteria(description) {
    if (!description) return null;

    // Look for "Acceptance Criteria:" section
    const acMatch = description.match(/Acceptance Criteria:?\s*\n([\s\S]*?)(?=\n##|\n\n\n|$)/i);
    if (acMatch) {
      return acMatch[1].trim();
    }

    // Look for checklist items that might be AC
    const checklistMatch = description.match(/(?:^|\n)((?:\s*[-*]\s*\[[ x]\].*\n?)+)/i);
    if (checklistMatch) {
      return checklistMatch[1].trim();
    }

    return null;
  }
}

module.exports = BeadsProvider;
