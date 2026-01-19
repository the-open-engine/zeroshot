/**
 * Azure DevOps Provider - Fetch work items via az CLI
 * Supports Azure DevOps cloud (dev.azure.com)
 */

const IssueProvider = require('./base-provider');
const { execSync } = require('../lib/safe-exec');
const { detectGitContext } = require('../../lib/git-remote-utils');

const AUTH_CHECK_TIMEOUT_MS = 2000;

class AzureDevOpsProvider extends IssueProvider {
  static id = 'azure-devops';
  static displayName = 'Azure DevOps';

  /**
   * Azure DevOps supports PR creation via az CLI
   */
  static supportsPR() {
    return true;
  }

  /**
   * Get PR CLI tool info for preflight checks
   */
  static getPRTool() {
    return {
      name: 'az',
      checkCmd: 'az --version',
      installHint: 'https://docs.microsoft.com/cli/azure/',
      displayName: 'Azure DevOps',
    };
  }

  /**
   * Detect Azure DevOps work item URLs
   * Matches:
   * - dev.azure.com URLs
   * - *.visualstudio.com URLs
   * - Bare numbers when git remote is Azure DevOps (auto-detected)
   * - Bare numbers when defaultIssueSource=azure-devops (requires azureOrg setting)
   *
   * @param {string} input - Issue identifier (URL or number)
   * @param {Object} settings - User settings
   * @param {Object|null} gitContext - Auto-detected git remote context
   * @returns {boolean} True if this provider should handle the input
   */
  static detectIdentifier(input, settings, gitContext = null) {
    // dev.azure.com URLs
    if (/dev\.azure\.com\/.*\/_workitems\/edit\/\d+/.test(input)) {
      return true;
    }

    // Legacy visualstudio.com URLs (e.g., https://org.visualstudio.com/project/_workitems/edit/123)
    if (/https?:\/\/[^.]+\.visualstudio\.com\/[^/]+\/_workitems\/edit\/\d+/.test(input)) {
      return true;
    }

    // Bare numbers - use shared priority cascade logic
    // Azure DevOps requires azureOrg setting when using defaultIssueSource
    return IssueProvider.detectBareNumber(input, settings, gitContext, 'azure-devops', {
      requiredSettings: ['azureOrg'],
    });
  }

  static getRequiredTool() {
    return {
      name: 'az',
      checkCmd: 'az --version',
      installHint:
        'Install Azure CLI: https://docs.microsoft.com/en-us/cli/azure/install-azure-cli\nThen: az extension add --name azure-devops',
    };
  }

  /**
   * Check az CLI authentication for Azure DevOps
   */
  static checkAuth() {
    try {
      // First check Azure login
      execSync('az account show', {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: AUTH_CHECK_TIMEOUT_MS,
      });
    } catch (err) {
      const stderr = err.stderr || err.message || '';

      if (err.code === 'ENOENT' || stderr.includes('command not found')) {
        return {
          authenticated: false,
          error: 'Azure CLI not installed',
          recovery: [
            'Install Azure CLI: https://docs.microsoft.com/cli/azure/',
            'Then verify: az --version',
          ],
        };
      }

      if (stderr.includes('Command timed out')) {
        return {
          authenticated: false,
          error: 'az account show timed out',
          recovery: ['Retry: az account show', 'If it still hangs, run: az login'],
        };
      }

      if (stderr.includes('az login') || stderr.includes('not logged in')) {
        return {
          authenticated: false,
          error: 'Azure CLI not authenticated',
          recovery: [
            'Run: az login',
            'Then verify: az account show',
            'Ensure azure-devops extension: az extension add --name azure-devops',
          ],
        };
      }

      return {
        authenticated: false,
        error: stderr.trim() || 'Unknown Azure CLI error',
        recovery: ['Run: az login', 'Then verify: az account show'],
      };
    }

    // Check azure-devops extension is installed
    try {
      const output = execSync('az extension list --query "[?name==\'azure-devops\']" -o json', {
        encoding: 'utf8',
        stdio: 'pipe',
        timeout: AUTH_CHECK_TIMEOUT_MS,
      });
      const extensions = JSON.parse(output);
      if (extensions.length === 0) {
        return {
          authenticated: false,
          error: 'Azure DevOps extension not installed',
          recovery: [
            'Install the extension: az extension add --name azure-devops',
            'Then verify: az extension list | grep azure-devops',
          ],
        };
      }
    } catch (err) {
      const stderr = err?.stderr || err?.message || '';
      if (stderr.includes('Command timed out')) {
        return {
          authenticated: false,
          error: 'az extension list timed out',
          recovery: ['Retry: az extension list', 'Ensure az is configured and responsive'],
        };
      }
      return {
        authenticated: false,
        error: 'Could not verify Azure DevOps extension',
        recovery: ['Install the extension: az extension add --name azure-devops'],
      };
    }

    return { authenticated: true, error: null, recovery: [] };
  }

  /**
   * Azure DevOps-specific settings schema
   */
  static getSettingsSchema() {
    return {
      azureOrg: {
        type: 'string',
        nullable: true,
        default: null,
        description: "Azure DevOps org URL (e.g., 'https://dev.azure.com/myorg')",
      },
      azureProject: {
        type: 'string',
        nullable: true,
        default: null,
        description: "Azure DevOps project name (e.g., 'MyProject')",
      },
    };
  }

  fetchIssue(identifier, settings) {
    try {
      const { workItemId, org, project: _project } = this._parseIdentifier(identifier, settings);

      if (!org) {
        throw new Error(
          'Azure DevOps requires azureOrg setting. Set via: zeroshot settings set azureOrg <url>'
        );
      }

      // Fetch work item using az CLI
      // Note: Work item IDs are unique per organization, no --project flag needed
      const cmd = `az boards work-item show --id ${workItemId} --org "${org}" -o json`;
      const output = execSync(cmd, { encoding: 'utf8' });
      const workItem = JSON.parse(output);

      return this._parseWorkItem(workItem);
    } catch (error) {
      throw new Error(`Failed to fetch Azure DevOps work item: ${error.message}`);
    }
  }

  /**
   * Parse identifier to extract work item ID, org, and project
   * Uses git context for auto-detection when settings not provided
   * @private
   */
  _parseIdentifier(identifier, settings) {
    // URL format: https://dev.azure.com/org/project/_workitems/edit/123
    // Use [^/]+ to match any characters except /, supporting spaces and special chars
    const urlMatch = identifier.match(/dev\.azure\.com\/([^/]+)\/([^/]+)\/_workitems\/edit\/(\d+)/);
    if (urlMatch) {
      return {
        org: `https://dev.azure.com/${urlMatch[1]}`,
        project: decodeURIComponent(urlMatch[2]), // Decode URL-encoded project names
        workItemId: urlMatch[3],
      };
    }

    // Legacy URL format: https://org.visualstudio.com/project/_workitems/edit/123
    const legacyMatch = identifier.match(
      /https?:\/\/([^.]+)\.visualstudio\.com\/([^/]+)\/_workitems\/edit\/(\d+)/
    );
    if (legacyMatch) {
      return {
        org: `https://${legacyMatch[1]}.visualstudio.com`,
        project: decodeURIComponent(legacyMatch[2]), // Decode URL-encoded project names
        workItemId: legacyMatch[3],
      };
    }

    // Bare number: try settings first, then git context
    if (/^\d+$/.test(identifier)) {
      let org = settings.azureOrg;
      let project = settings.azureProject;

      // If settings don't have org, try git context
      if (!org) {
        const gitContext = detectGitContext();
        if (gitContext?.provider === 'azure-devops') {
          org = gitContext.azureOrg;
          project = gitContext.azureProject;
        }
      }

      return {
        org,
        project,
        workItemId: identifier,
      };
    }

    throw new Error(`Could not parse Azure DevOps work item identifier: ${identifier}`);
  }

  /**
   * Parse Azure DevOps work item into standardized InputData format
   * @private
   */
  _parseWorkItem(workItem) {
    const fields = workItem.fields || {};
    const id = workItem.id;
    const title = fields['System.Title'] || '';
    const description = fields['System.Description'] || '';
    const tags = fields['System.Tags'] ? fields['System.Tags'].split(';').map((t) => t.trim()) : [];

    let context = `# Azure DevOps Work Item #${id}\n\n`;
    context += `## Title\n${title}\n\n`;

    if (description) {
      // Strip HTML tags from description (Azure stores as HTML)
      const plainDescription = description.replace(/<[^>]*>/g, '');
      context += `## Description\n${plainDescription}\n\n`;
    }

    if (tags.length > 0) {
      context += `## Tags\n`;
      context += tags.map((t) => `- ${t}`).join('\n');
      context += '\n\n';
    }

    // Map tags to labels format
    const labels = tags.map((name) => ({ name }));

    // Azure work items don't have comments in the same API call
    // Would need separate call to fetch comments, omitting for now
    const comments = [];

    // Strip HTML from description for body field
    const plainBody = description.replace(/<[^>]*>/g, '');

    return {
      number: id,
      title,
      body: plainBody,
      labels,
      comments,
      url: workItem.url || null,
      context,
    };
  }
}

module.exports = AzureDevOpsProvider;
