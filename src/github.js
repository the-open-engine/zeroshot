/**
 * GitHub - Fetch and parse GitHub issues
 *
 * Provides:
 * - Issue fetching via gh CLI
 * - Parsing of issue data into context
 * - Fallback to plain text input
 */

const { execSync } = require('child_process');

class GitHub {
  /**
   * Fetch GitHub issue by URL or number
   * @param {String} issueRef - Issue URL or number
   * @returns {Object} Parsed issue data
   */
  static fetchIssue(issueRef) {
    try {
      // Extract issue number from URL if needed
      const issueNumber = this._extractIssueNumber(issueRef);

      // Fetch issue using gh CLI
      const cmd = `gh issue view ${issueNumber} --json number,title,body,labels,assignees,comments,url`;
      const output = execSync(cmd, { encoding: 'utf8' });
      const issue = JSON.parse(output);

      return this._parseIssue(issue);
    } catch (error) {
      throw new Error(`Failed to fetch GitHub issue: ${error.message}`);
    }
  }

  /**
   * Extract issue number from URL or return as-is
   * @private
   */
  static _extractIssueNumber(issueRef) {
    // If it's a URL, extract the number
    const urlMatch = issueRef.match(/\/issues\/(\d+)/);
    if (urlMatch) {
      return urlMatch[1];
    }

    // Otherwise assume it's already a number
    return issueRef;
  }

  /**
   * Parse issue into structured context
   * @private
   */
  static _parseIssue(issue) {
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

  /**
   * Create a plain text input wrapper
   * @param {String} text - Plain text input
   * @returns {Object} Structured context
   */
  static createTextInput(text) {
    return {
      number: null,
      title: 'Manual Input',
      body: text,
      labels: [],
      comments: [],
      context: `# Manual Input\n\n${text}\n`,
    };
  }
}

module.exports = GitHub;
