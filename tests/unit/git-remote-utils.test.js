/**
 * Test: Git Remote URL Parsing and Provider Detection
 *
 * Tests automatic provider detection from git remote URLs:
 * - Parse GitHub, GitLab, Azure DevOps URLs (HTTPS + SSH)
 * - Extract org/repo/project information
 * - Detect git context from working directory
 * - Graceful error handling
 */

const assert = require('assert');
const { parseGitRemoteUrl, detectGitContext } = require('../../lib/git-remote-utils');

describe('Git Remote Utils', function () {
  registerParseGitRemoteUrlTests();
  registerDetectGitContextTests();
});

function registerParseGitRemoteUrlTests() {
  describe('parseGitRemoteUrl', function () {
    registerGitHubParsingTests();
    registerGitLabParsingTests();
    registerAzureDevOpsParsingTests();
    registerEdgeCaseTests();
  });
}

function registerGitHubParsingTests() {
  describe('GitHub URLs', function () {
    it('should parse GitHub HTTPS URL', function () {
      const result = parseGitRemoteUrl('https://github.com/covibes/zeroshot.git');
      assert.strictEqual(result.provider, 'github');
      assert.strictEqual(result.host, 'github.com');
      assert.strictEqual(result.org, 'covibes');
      assert.strictEqual(result.repo, 'zeroshot');
      assert.strictEqual(result.fullRepo, 'covibes/zeroshot');
    });

    it('should parse GitHub HTTPS URL without .git suffix', function () {
      const result = parseGitRemoteUrl('https://github.com/facebook/react');
      assert.strictEqual(result.provider, 'github');
      assert.strictEqual(result.org, 'facebook');
      assert.strictEqual(result.repo, 'react');
      assert.strictEqual(result.fullRepo, 'facebook/react');
    });

    it('should parse GitHub SSH URL', function () {
      const result = parseGitRemoteUrl('git@github.com:microsoft/vscode.git');
      assert.strictEqual(result.provider, 'github');
      assert.strictEqual(result.host, 'github.com');
      assert.strictEqual(result.org, 'microsoft');
      assert.strictEqual(result.repo, 'vscode');
      assert.strictEqual(result.fullRepo, 'microsoft/vscode');
    });

    it('should parse GitHub SSH URL without .git suffix', function () {
      const result = parseGitRemoteUrl('git@github.com:torvalds/linux');
      assert.strictEqual(result.provider, 'github');
      assert.strictEqual(result.org, 'torvalds');
      assert.strictEqual(result.repo, 'linux');
    });
  });
}

function registerGitLabParsingTests() {
  describe('GitLab URLs', function () {
    it('should parse GitLab cloud HTTPS URL', function () {
      const result = parseGitRemoteUrl('https://gitlab.com/gitlab-org/gitlab.git');
      assert.strictEqual(result.provider, 'gitlab');
      assert.strictEqual(result.host, 'gitlab.com');
      assert.strictEqual(result.org, 'gitlab-org');
      assert.strictEqual(result.repo, 'gitlab');
      assert.strictEqual(result.fullRepo, 'gitlab-org/gitlab');
    });

    it('should parse GitLab cloud SSH URL', function () {
      const result = parseGitRemoteUrl('git@gitlab.com:mycompany/myproject.git');
      assert.strictEqual(result.provider, 'gitlab');
      assert.strictEqual(result.host, 'gitlab.com');
      assert.strictEqual(result.org, 'mycompany');
      assert.strictEqual(result.repo, 'myproject');
      assert.strictEqual(result.fullRepo, 'mycompany/myproject');
    });

    it('should parse self-hosted GitLab HTTPS URL', function () {
      const result = parseGitRemoteUrl('https://gitlab.company.com/team/repo.git');
      assert.strictEqual(result.provider, 'gitlab');
      assert.strictEqual(result.host, 'gitlab.company.com');
      assert.strictEqual(result.org, 'team');
      assert.strictEqual(result.repo, 'repo');
      assert.strictEqual(result.fullRepo, 'team/repo');
    });

    it('should parse self-hosted GitLab SSH URL', function () {
      const result = parseGitRemoteUrl('git@gitlab.company.com:team/repo.git');
      assert.strictEqual(result.provider, 'gitlab');
      assert.strictEqual(result.host, 'gitlab.company.com');
      assert.strictEqual(result.org, 'team');
      assert.strictEqual(result.repo, 'repo');
    });

    it('should detect gitlab in subdomain', function () {
      const result = parseGitRemoteUrl('https://my-gitlab.example.com/org/repo.git');
      assert.strictEqual(result.provider, 'gitlab');
      assert.strictEqual(result.host, 'my-gitlab.example.com');
    });
  });
}

function registerAzureDevOpsParsingTests() {
  describe('Azure DevOps URLs', function () {
    it('should parse Azure DevOps HTTPS URL', function () {
      const result = parseGitRemoteUrl('https://dev.azure.com/myorg/myproject/_git/myrepo');
      assert.strictEqual(result.provider, 'azure-devops');
      assert.strictEqual(result.host, 'dev.azure.com');
      assert.strictEqual(result.azureOrg, 'https://dev.azure.com/myorg');
      assert.strictEqual(result.azureProject, 'myproject');
      assert.strictEqual(result.repo, 'myrepo');
    });

    it('should parse Azure DevOps HTTPS URL with .git suffix', function () {
      const result = parseGitRemoteUrl('https://dev.azure.com/company/project/_git/repo.git');
      assert.strictEqual(result.provider, 'azure-devops');
      assert.strictEqual(result.azureOrg, 'https://dev.azure.com/company');
      assert.strictEqual(result.azureProject, 'project');
      assert.strictEqual(result.repo, 'repo');
    });

    it('should parse Azure legacy visualstudio.com URL', function () {
      const result = parseGitRemoteUrl('https://myorg.visualstudio.com/myproject/_git/myrepo');
      assert.strictEqual(result.provider, 'azure-devops');
      assert.strictEqual(result.host, 'myorg.visualstudio.com');
      assert.strictEqual(result.azureOrg, 'https://myorg.visualstudio.com');
      assert.strictEqual(result.azureProject, 'myproject');
      assert.strictEqual(result.repo, 'myrepo');
    });

    it('should parse Azure SSH URL', function () {
      const result = parseGitRemoteUrl('git@ssh.dev.azure.com:v3/myorg/myproject/myrepo');
      assert.strictEqual(result.provider, 'azure-devops');
      assert.strictEqual(result.azureOrg, 'https://dev.azure.com/myorg');
      assert.strictEqual(result.azureProject, 'myproject');
      assert.strictEqual(result.repo, 'myrepo');
    });
  });
}

function registerEdgeCaseTests() {
  describe('Edge Cases', function () {
    it('should return null for empty string', function () {
      const result = parseGitRemoteUrl('');
      assert.strictEqual(result, null);
    });

    it('should return null for null input', function () {
      const result = parseGitRemoteUrl(null);
      assert.strictEqual(result, null);
    });

    it('should return null for undefined input', function () {
      const result = parseGitRemoteUrl(undefined);
      assert.strictEqual(result, null);
    });

    it('should return null for non-string input', function () {
      const result = parseGitRemoteUrl(123);
      assert.strictEqual(result, null);
    });

    it('should return null for invalid URL format', function () {
      const result = parseGitRemoteUrl('not-a-valid-url');
      assert.strictEqual(result, null);
    });

    it('should return null for unknown git hosting service', function () {
      const result = parseGitRemoteUrl('https://unknown-service.com/org/repo.git');
      assert.strictEqual(result, null);
    });

    it('should handle URL with trailing whitespace', function () {
      const result = parseGitRemoteUrl('  https://github.com/org/repo.git  ');
      assert.strictEqual(result.provider, 'github');
      assert.strictEqual(result.org, 'org');
      assert.strictEqual(result.repo, 'repo');
    });

    it('should handle SSH URL with special characters in repo name', function () {
      const result = parseGitRemoteUrl('git@github.com:org/my-repo.name.git');
      assert.strictEqual(result.provider, 'github');
      assert.strictEqual(result.repo, 'my-repo.name');
    });

    it('should handle HTTPS URL without protocol', function () {
      const result = parseGitRemoteUrl('github.com/org/repo.git');
      // Should return null because it doesn't match HTTPS pattern
      assert.strictEqual(result, null);
    });
  });
}

function registerDetectGitContextTests() {
  describe('detectGitContext', function () {
    it('should return null in non-git directory', function () {
      // /tmp is typically not a git repo
      const result = detectGitContext('/tmp');
      assert.strictEqual(result, null);
    });

    it('should return null with invalid directory', function () {
      const result = detectGitContext('/nonexistent/directory/path');
      assert.strictEqual(result, null);
    });

    // NOTE: Additional integration-style tests for detectGitContext
    // would require mocking git commands or setting up test fixtures.
    // These are better suited for integration tests.
    //
    // Unit tests above verify the core URL parsing logic in isolation.
    // Provider integration tests will verify the full workflow.
  });
}
