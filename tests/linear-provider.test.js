/**
 * Tests for Linear issue provider
 */

const { expect } = require('chai');
const { detectProvider, getProvider, listProviders } = require('../src/issue-providers');
const JiraProvider = require('../src/issue-providers/jira-provider');
const LinearProvider = require('../src/issue-providers/linear-provider');

describe('Provider Registry (Linear)', () => {
  it('listProviders includes linear', () => {
    expect(listProviders()).to.include('linear');
  });

  it('getProvider returns LinearProvider', () => {
    expect(getProvider('linear')).to.equal(LinearProvider);
  });
});

describe('Linear Provider', () => {
  describe('detectIdentifier', () => {
    it('detects Linear issue URLs', () => {
      expect(
        LinearProvider.detectIdentifier(
          'https://linear.app/my-workspace/issue/ENG-42/some-title',
          {}
        )
      ).to.be.true;
    });

    it('detects Linear issue keys', () => {
      expect(LinearProvider.detectIdentifier('ENG-42', {})).to.be.true;
      expect(LinearProvider.detectIdentifier('A1-789', {})).to.be.true;
    });

    it('rejects invalid key formats', () => {
      expect(LinearProvider.detectIdentifier('eng-42', {})).to.be.false; // lowercase
      expect(LinearProvider.detectIdentifier('123', {})).to.be.false; // no key, no settings
      expect(LinearProvider.detectIdentifier('ENG42', {})).to.be.false; // no dash
    });

    it('rejects non-Linear URLs', () => {
      expect(LinearProvider.detectIdentifier('https://gitlab.com/org/repo/-/issues/123', {})).to.be
        .false;
    });

    it('detects bare numbers when Linear is default with linearTeam', () => {
      const settings = { defaultIssueSource: 'linear', linearTeam: 'ENG' };
      expect(LinearProvider.detectIdentifier('42', settings)).to.be.true;
    });

    it('rejects bare numbers when Linear is default without linearTeam', () => {
      const settings = { defaultIssueSource: 'linear' };
      expect(LinearProvider.detectIdentifier('42', settings)).to.be.false;
    });

    it('rejects bare numbers with no settings at all', () => {
      expect(LinearProvider.detectIdentifier('42', {})).to.be.false;
    });
  });

  describe('Jira/Linear key ambiguity', () => {
    it('resolves ambiguous KEY-NUMBER to Jira by registration order', () => {
      const ProviderClass = detectProvider('ENG-42', {}, null, null);
      expect(ProviderClass).to.equal(JiraProvider);
    });

    it('force flag selects Linear despite ambiguity', () => {
      const ProviderClass = detectProvider('ENG-42', {}, 'linear', null);
      expect(ProviderClass).to.equal(LinearProvider);
    });
  });

  it('getRequiredTool signals no CLI binary required', () => {
    const tool = LinearProvider.getRequiredTool();
    expect(tool.name).to.be.null;
    expect(tool.checkCmd).to.be.null;
  });

  describe('_extractIssueKey', () => {
    const provider = new LinearProvider();

    it('extracts key from URL', () => {
      expect(
        provider._extractIssueKey('https://linear.app/ws/issue/ENG-42/some-title', {})
      ).to.equal('ENG-42');
    });

    it('builds key from bare number + linearTeam', () => {
      expect(provider._extractIssueKey('42', { linearTeam: 'ENG' })).to.equal('ENG-42');
    });

    it('passes through an already-formed key', () => {
      expect(provider._extractIssueKey('ENG-42', {})).to.equal('ENG-42');
    });
  });

  describe('fetchIssue', () => {
    let originalFetch;

    beforeEach(() => {
      originalFetch = global.fetch;
    });

    afterEach(() => {
      global.fetch = originalFetch;
    });

    it('returns the standardized InputData shape', async () => {
      global.fetch = () => ({
        status: 200,
        json: () => ({
          data: {
            issue: {
              identifier: 'ENG-42',
              number: 42,
              title: 'Fix the thing',
              description: 'Detailed description',
              url: 'https://linear.app/ws/issue/ENG-42',
              labels: { nodes: [{ name: 'bug' }] },
              comments: {
                nodes: [
                  {
                    user: { name: 'Alice' },
                    createdAt: '2026-01-01T00:00:00.000Z',
                    body: 'Looks good',
                  },
                ],
              },
            },
          },
        }),
      });

      const provider = new LinearProvider();
      const result = await provider.fetchIssue('ENG-42', {});

      expect(result.number).to.equal(42);
      expect(result.title).to.equal('Fix the thing');
      expect(result.body).to.equal('Detailed description');
      expect(result.labels).to.deep.equal([{ name: 'bug' }]);
      expect(result.comments).to.deep.equal([
        { author: { login: 'Alice' }, createdAt: '2026-01-01T00:00:00.000Z', body: 'Looks good' },
      ]);
      expect(result.url).to.equal('https://linear.app/ws/issue/ENG-42');
      expect(result.context).to.include('# Linear Issue ENG-42');
      expect(result.context).to.include('## Title');
      expect(result.context).to.include('## Description');
      expect(result.context).to.include('## Labels');
      expect(result.context).to.include('## Comments');
    });

    it('throws a descriptive error on GraphQL errors', async () => {
      global.fetch = () => ({
        status: 200,
        json: () => ({ errors: [{ message: 'Entity not found' }] }),
      });

      const provider = new LinearProvider();
      try {
        await provider.fetchIssue('ENG-999', {});
        expect.fail('should have thrown');
      } catch (err) {
        expect(err.message).to.include('Failed to fetch Linear issue');
        expect(err.message).to.include('Entity not found');
      }
    });
  });

  describe('checkAuth', () => {
    let originalFetch;
    let originalApiKey;

    beforeEach(() => {
      originalFetch = global.fetch;
      originalApiKey = process.env.LINEAR_API_KEY;
    });

    afterEach(() => {
      global.fetch = originalFetch;
      if (originalApiKey === undefined) {
        delete process.env.LINEAR_API_KEY;
      } else {
        process.env.LINEAR_API_KEY = originalApiKey;
      }
    });

    it('fails fast with recovery steps when LINEAR_API_KEY is not set', async () => {
      delete process.env.LINEAR_API_KEY;
      const result = await LinearProvider.checkAuth();
      expect(result.authenticated).to.be.false;
      expect(result.error).to.equal('LINEAR_API_KEY not set');
      expect(result.recovery).to.have.lengthOf(2);
    });

    it('succeeds with a valid key', async () => {
      process.env.LINEAR_API_KEY = 'lin_api_valid';
      global.fetch = () => ({
        status: 200,
        json: () => ({ data: { viewer: { id: 'user_1' } } }),
      });

      const result = await LinearProvider.checkAuth();
      expect(result.authenticated).to.be.true;
      expect(result.error).to.be.null;
    });

    it('fails on 401 response', async () => {
      process.env.LINEAR_API_KEY = 'lin_api_invalid';
      global.fetch = () => ({
        status: 401,
        json: () => ({}),
      });

      const result = await LinearProvider.checkAuth();
      expect(result.authenticated).to.be.false;
      expect(result.error).to.equal('Linear API key invalid');
    });

    it('fails on GraphQL errors', async () => {
      process.env.LINEAR_API_KEY = 'lin_api_invalid';
      global.fetch = () => ({
        status: 200,
        json: () => ({ errors: [{ message: 'Authentication required' }] }),
      });

      const result = await LinearProvider.checkAuth();
      expect(result.authenticated).to.be.false;
      expect(result.error).to.equal('Linear API key invalid');
    });
  });

  describe('getSettingsSchema / validateSetting', () => {
    it('accepts a valid linearTeam key', () => {
      expect(LinearProvider.validateSetting('linearTeam', 'ENG')).to.be.null;
    });

    it('rejects a lowercase linearTeam key', () => {
      expect(LinearProvider.validateSetting('linearTeam', 'eng')).to.be.a('string');
    });

    it('rejects a numeric linearTeam key', () => {
      expect(LinearProvider.validateSetting('linearTeam', '123')).to.be.a('string');
    });
  });
});
