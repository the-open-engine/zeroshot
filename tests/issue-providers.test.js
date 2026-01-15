/**
 * Tests for issue provider system
 */

const { expect } = require('chai');
const { detectProvider, getProvider, listProviders } = require('../src/issue-providers');
const GitHubProvider = require('../src/issue-providers/github-provider');
const GitLabProvider = require('../src/issue-providers/gitlab-provider');
const JiraProvider = require('../src/issue-providers/jira-provider');
const AzureDevOpsProvider = require('../src/issue-providers/azure-devops-provider');

describe('Provider Registry', () => {
  it('listProviders returns all registered providers', () => {
    const providers = listProviders();
    expect(providers).to.include('github');
    expect(providers).to.include('gitlab');
    expect(providers).to.include('jira');
    expect(providers).to.include('azure-devops');
  });

  it('getProvider returns provider by ID', () => {
    expect(getProvider('github')).to.equal(GitHubProvider);
    expect(getProvider('gitlab')).to.equal(GitLabProvider);
    expect(getProvider('jira')).to.equal(JiraProvider);
    expect(getProvider('azure-devops')).to.equal(AzureDevOpsProvider);
  });

  it('getProvider returns null for unknown provider', () => {
    expect(getProvider('unknown')).to.be.null;
  });
});

describe('GitHub Provider', () => {
  const settings = { defaultIssueSource: 'github' };

  describe('detectIdentifier', () => {
    it('detects GitHub URLs', () => {
      expect(GitHubProvider.detectIdentifier('https://github.com/org/repo/issues/123', settings)).to
        .be.true;
    });

    it('detects org/repo#123 format', () => {
      expect(GitHubProvider.detectIdentifier('org/repo#123', settings)).to.be.true;
    });

    it('detects bare numbers when GitHub is default', () => {
      expect(GitHubProvider.detectIdentifier('123', settings)).to.be.true;
    });

    it('detects bare numbers when no default set', () => {
      expect(GitHubProvider.detectIdentifier('123', {})).to.be.true;
    });

    it('rejects non-GitHub URLs', () => {
      expect(GitHubProvider.detectIdentifier('https://gitlab.com/org/repo/-/issues/123', settings))
        .to.be.false;
    });
  });

  it('getRequiredTool returns gh', () => {
    const tool = GitHubProvider.getRequiredTool();
    expect(tool.name).to.equal('gh');
    expect(tool.checkCmd).to.equal('gh --version');
  });
});

describe('GitLab Provider', () => {
  describe('detectIdentifier', () => {
    it('detects GitLab cloud URLs', () => {
      const settings = {};
      expect(GitLabProvider.detectIdentifier('https://gitlab.com/org/repo/-/issues/123', settings))
        .to.be.true;
    });

    it('detects self-hosted GitLab URLs', () => {
      const settings = { gitlabInstance: 'gitlab.company.com' };
      expect(
        GitLabProvider.detectIdentifier(
          'https://gitlab.company.com/org/repo/-/issues/123',
          settings
        )
      ).to.be.true;
    });

    it('detects org/repo#123 when GitLab is default', () => {
      const settings = { defaultIssueSource: 'gitlab' };
      expect(GitLabProvider.detectIdentifier('org/repo#123', settings)).to.be.true;
    });

    it('detects bare numbers when GitLab is default', () => {
      const settings = { defaultIssueSource: 'gitlab' };
      expect(GitLabProvider.detectIdentifier('123', settings)).to.be.true;
    });

    it('rejects GitHub URLs', () => {
      expect(GitLabProvider.detectIdentifier('https://github.com/org/repo/issues/123', {})).to.be
        .false;
    });
  });

  it('getRequiredTool returns glab', () => {
    const tool = GitLabProvider.getRequiredTool();
    expect(tool.name).to.equal('glab');
    expect(tool.checkCmd).to.equal('glab --version');
  });
});

describe('Jira Provider', () => {
  describe('detectIdentifier', () => {
    it('detects Jira Cloud URLs', () => {
      const settings = {};
      expect(
        JiraProvider.detectIdentifier('https://company.atlassian.net/browse/PROJ-123', settings)
      ).to.be.true;
    });

    it('detects self-hosted Jira URLs', () => {
      const settings = { jiraInstance: 'jira.company.com' };
      expect(JiraProvider.detectIdentifier('https://jira.company.com/browse/PROJ-123', settings)).to
        .be.true;
    });

    it('detects Jira issue keys', () => {
      const settings = {};
      expect(JiraProvider.detectIdentifier('PROJ-123', settings)).to.be.true;
      expect(JiraProvider.detectIdentifier('ABC-456', settings)).to.be.true;
      expect(JiraProvider.detectIdentifier('A1-789', settings)).to.be.true;
    });

    it('rejects invalid key formats', () => {
      expect(JiraProvider.detectIdentifier('proj-123', {})).to.be.false; // lowercase
      expect(JiraProvider.detectIdentifier('123', {})).to.be.false; // no key
      expect(JiraProvider.detectIdentifier('PROJ123', {})).to.be.false; // no dash
    });

    it('detects bare numbers when Jira is default with jiraProject', () => {
      const settings = { defaultIssueSource: 'jira', jiraProject: 'PROJ' };
      expect(JiraProvider.detectIdentifier('123', settings)).to.be.true;
    });

    it('rejects bare numbers when Jira is default without jiraProject', () => {
      const settings = { defaultIssueSource: 'jira' };
      expect(JiraProvider.detectIdentifier('123', settings)).to.be.false;
    });
  });

  it('getRequiredTool returns jira', () => {
    const tool = JiraProvider.getRequiredTool();
    expect(tool.name).to.equal('jira');
    expect(tool.checkCmd).to.equal('jira version');
  });
});

describe('Azure DevOps Provider', () => {
  describe('detectIdentifier', () => {
    it('detects Azure DevOps URLs', () => {
      const settings = {};
      expect(
        AzureDevOpsProvider.detectIdentifier(
          'https://dev.azure.com/org/proj/_workitems/edit/123',
          settings
        )
      ).to.be.true;
    });

    it('detects Azure DevOps URLs with mixed case project names', () => {
      const settings = {};
      expect(
        AzureDevOpsProvider.detectIdentifier(
          'https://dev.azure.com/wtsgroup/PlayGroundAI/_workitems/edit/2462',
          settings
        )
      ).to.be.true;
    });

    it('detects legacy Visual Studio URLs', () => {
      const settings = {};
      expect(
        AzureDevOpsProvider.detectIdentifier(
          'https://org.visualstudio.com/proj/_workitems/edit/123',
          settings
        )
      ).to.be.true;
    });

    it('detects bare numbers with Azure settings', () => {
      const settings = {
        defaultIssueSource: 'azure-devops',
        azureOrg: 'https://dev.azure.com/org',
      };
      expect(AzureDevOpsProvider.detectIdentifier('123', settings)).to.be.true;
    });

    it('rejects bare numbers without azureOrg setting', () => {
      const settings = { defaultIssueSource: 'azure-devops' };
      expect(AzureDevOpsProvider.detectIdentifier('123', settings)).to.be.false;
    });
  });

  describe('_parseIdentifier', () => {
    it('parses standard Azure DevOps URL', () => {
      const provider = new AzureDevOpsProvider();
      const result = provider._parseIdentifier(
        'https://dev.azure.com/org/proj/_workitems/edit/123',
        {}
      );
      expect(result.org).to.equal('https://dev.azure.com/org');
      expect(result.project).to.equal('proj');
      expect(result.workItemId).to.equal('123');
    });

    it('parses URL with mixed case project name', () => {
      const provider = new AzureDevOpsProvider();
      const result = provider._parseIdentifier(
        'https://dev.azure.com/wtsgroup/PlayGroundAI/_workitems/edit/2462',
        {}
      );
      expect(result.org).to.equal('https://dev.azure.com/wtsgroup');
      expect(result.project).to.equal('PlayGroundAI');
      expect(result.workItemId).to.equal('2462');
    });

    it('parses URL with URL-encoded project name', () => {
      const provider = new AzureDevOpsProvider();
      const result = provider._parseIdentifier(
        'https://dev.azure.com/org/My%20Project/_workitems/edit/456',
        {}
      );
      expect(result.org).to.equal('https://dev.azure.com/org');
      expect(result.project).to.equal('My Project');
      expect(result.workItemId).to.equal('456');
    });

    it('parses legacy Visual Studio URL', () => {
      const provider = new AzureDevOpsProvider();
      const result = provider._parseIdentifier(
        'https://myorg.visualstudio.com/myproject/_workitems/edit/789',
        {}
      );
      expect(result.org).to.equal('https://myorg.visualstudio.com');
      expect(result.project).to.equal('myproject');
      expect(result.workItemId).to.equal('789');
    });
  });

  it('getRequiredTool returns az', () => {
    const tool = AzureDevOpsProvider.getRequiredTool();
    expect(tool.name).to.equal('az');
    expect(tool.checkCmd).to.equal('az --version');
  });
});

describe('Git Remote Auto-Detection', () => {
  describe('GitHub Provider with git context', () => {
    it('detects bare numbers when git remote is GitHub', () => {
      const gitContext = {
        provider: 'github',
        org: 'myorg',
        repo: 'myrepo',
        fullRepo: 'myorg/myrepo',
      };
      expect(GitHubProvider.detectIdentifier('123', {}, gitContext)).to.be.true;
    });

    it('git context overrides legacy fallback', () => {
      const gitlabContext = { provider: 'gitlab', org: 'myorg', repo: 'myrepo' };
      // Without git context, GitHub would be the fallback
      // With GitLab git context, GitHub should NOT match
      expect(GitHubProvider.detectIdentifier('123', {}, gitlabContext)).to.be.false;
    });

    it('git context takes priority over settings', () => {
      const gitlabContext = { provider: 'gitlab' };
      const settings = { defaultIssueSource: 'github' };
      // Git context says GitLab, so GitHub should NOT match even though settings say GitHub
      expect(GitHubProvider.detectIdentifier('123', settings, gitlabContext)).to.be.false;
      expect(GitLabProvider.detectIdentifier('123', settings, gitlabContext)).to.be.true;
    });
  });

  describe('GitLab Provider with git context', () => {
    it('detects bare numbers when git remote is GitLab', () => {
      const gitContext = {
        provider: 'gitlab',
        org: 'myorg',
        repo: 'myrepo',
        fullRepo: 'myorg/myrepo',
      };
      expect(GitLabProvider.detectIdentifier('123', {}, gitContext)).to.be.true;
    });

    it('detects org/repo#123 when git remote is GitLab', () => {
      const gitContext = { provider: 'gitlab', org: 'myorg', repo: 'myrepo' };
      expect(GitLabProvider.detectIdentifier('org/repo#123', {}, gitContext)).to.be.true;
    });

    it('rejects when git remote is not GitLab', () => {
      const githubContext = { provider: 'github' };
      expect(GitLabProvider.detectIdentifier('123', {}, githubContext)).to.be.false;
    });
  });

  describe('Azure DevOps Provider with git context', () => {
    it('detects bare numbers when git remote is Azure', () => {
      const gitContext = {
        provider: 'azure-devops',
        azureOrg: 'https://dev.azure.com/myorg',
        azureProject: 'myproject',
        repo: 'myrepo',
      };
      expect(AzureDevOpsProvider.detectIdentifier('123', {}, gitContext)).to.be.true;
    });

    it('rejects when git remote is not Azure', () => {
      const githubContext = { provider: 'github' };
      expect(AzureDevOpsProvider.detectIdentifier('123', {}, githubContext)).to.be.false;
    });

    it('git context takes priority over settings', () => {
      const githubContext = { provider: 'github' };
      const settings = {
        defaultIssueSource: 'azure-devops',
        azureOrg: 'https://dev.azure.com/myorg',
      };
      // Git context says GitHub, so Azure should NOT match even though settings say Azure
      expect(AzureDevOpsProvider.detectIdentifier('123', settings, githubContext)).to.be.false;
    });
  });

  describe('Priority order verification', () => {
    it('git context takes priority over legacy fallback', () => {
      const gitlabContext = { provider: 'gitlab' };
      // Without settings and with GitLab git context, should use GitLab not GitHub fallback
      expect(GitHubProvider.detectIdentifier('123', {}, gitlabContext)).to.be.false;
      expect(GitLabProvider.detectIdentifier('123', {}, gitlabContext)).to.be.true;
    });

    it('git context takes priority over settings', () => {
      const gitlabContext = { provider: 'gitlab' };
      const settings = { defaultIssueSource: 'github' };
      // Git context says GitLab, so GitLab should match even though settings say GitHub
      expect(GitHubProvider.detectIdentifier('123', settings, gitlabContext)).to.be.false;
      expect(GitLabProvider.detectIdentifier('123', settings, gitlabContext)).to.be.true;
    });

    it('null git context falls back gracefully', () => {
      // Should work same as before when git context is null
      expect(GitHubProvider.detectIdentifier('123', {}, null)).to.be.true; // GitHub fallback
      expect(GitLabProvider.detectIdentifier('123', { defaultIssueSource: 'gitlab' }, null)).to.be
        .true;
    });
  });
});

describe('Provider Auth Interface', () => {
  it('all providers have checkAuth method', () => {
    expect(GitHubProvider.checkAuth).to.be.a('function');
    expect(GitLabProvider.checkAuth).to.be.a('function');
    expect(JiraProvider.checkAuth).to.be.a('function');
    expect(AzureDevOpsProvider.checkAuth).to.be.a('function');
  });

  it('checkAuth returns proper structure', () => {
    // Mock by just checking structure - actual auth checks depend on environment
    const providers = [GitHubProvider, GitLabProvider, JiraProvider, AzureDevOpsProvider];

    for (const Provider of providers) {
      const result = Provider.checkAuth();
      expect(result).to.have.property('authenticated').that.is.a('boolean');
      expect(result).to.have.property('error');
      expect(result).to.have.property('recovery').that.is.an('array');

      // If authenticated, error should be null and recovery empty
      if (result.authenticated) {
        expect(result.error).to.be.null;
        expect(result.recovery).to.be.empty;
      } else {
        // If not authenticated, error should be a string and recovery should have steps
        expect(result.error).to.be.a('string');
        expect(result.recovery.length).to.be.greaterThan(0);
      }
    }
  });
});

describe('detectProvider', () => {
  it('force flag takes precedence', () => {
    const ProviderClass = detectProvider('123', {}, 'gitlab');
    expect(ProviderClass).to.equal(GitLabProvider);
  });

  it('detects GitHub by URL', () => {
    const ProviderClass = detectProvider('https://github.com/org/repo/issues/123', {});
    expect(ProviderClass).to.equal(GitHubProvider);
  });

  it('detects GitLab by URL', () => {
    const ProviderClass = detectProvider('https://gitlab.com/org/repo/-/issues/123', {});
    expect(ProviderClass).to.equal(GitLabProvider);
  });

  it('detects Jira by key', () => {
    const ProviderClass = detectProvider('PROJ-123', {});
    expect(ProviderClass).to.equal(JiraProvider);
  });

  it('detects Azure DevOps by URL', () => {
    const ProviderClass = detectProvider('https://dev.azure.com/org/proj/_workitems/edit/123', {});
    expect(ProviderClass).to.equal(AzureDevOpsProvider);
  });

  it('defaults to GitHub for bare numbers (no git context)', () => {
    // Pass null gitContext to test fallback behavior without git remote influence
    const ProviderClass = detectProvider('123', {}, null, null);
    expect(ProviderClass).to.equal(GitHubProvider);
  });

  it('uses defaultIssueSource setting (no git context)', () => {
    // Pass null gitContext to test settings-based detection without git remote influence
    const ProviderClass = detectProvider('123', { defaultIssueSource: 'gitlab' }, null, null);
    expect(ProviderClass).to.equal(GitLabProvider);
  });

  it('returns null for unmatched input', () => {
    const ProviderClass = detectProvider('not-an-issue', {});
    expect(ProviderClass).to.be.null;
  });
});
