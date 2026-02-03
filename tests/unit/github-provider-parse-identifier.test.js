/**
 * Regression test: GitHub provider must ALWAYS extract repo from identifier
 *
 * Bug: _extractIssueNumber discarded repo, causing `gh issue view` to guess
 * from CWD git remote, which failed when CWD was in a different repo.
 */

const { expect } = require('chai');
const GitHubProvider = require('../../src/issue-providers/github-provider');

describe('GitHubProvider._parseIdentifier', () => {
  let provider;

  beforeEach(() => {
    provider = new GitHubProvider();
  });

  describe('extracts repo from identifier', () => {
    it('org/repo#123 format returns both repo and number', () => {
      const result = provider._parseIdentifier('covibes/covibes#1172');
      expect(result).to.deep.equal({ repo: 'covibes/covibes', number: '1172' });
    });

    it('org-with-dash/repo-with-dash#123 format', () => {
      const result = provider._parseIdentifier('my-org/my-repo#456');
      expect(result).to.deep.equal({ repo: 'my-org/my-repo', number: '456' });
    });

    it('org.with.dots/repo.with.dots#123 format', () => {
      const result = provider._parseIdentifier('my.org/my.repo#789');
      expect(result).to.deep.equal({ repo: 'my.org/my.repo', number: '789' });
    });

    it('GitHub URL extracts repo and number', () => {
      const result = provider._parseIdentifier('https://github.com/covibes/covibes/issues/1172');
      expect(result).to.deep.equal({ repo: 'covibes/covibes', number: '1172' });
    });

    it('bare number with gitContext uses context repo', () => {
      const gitContext = { owner: 'covibes', repo: 'covibes' };
      const result = provider._parseIdentifier('1172', gitContext);
      expect(result).to.deep.equal({ repo: 'covibes/covibes', number: '1172' });
    });

    it('bare number without gitContext returns null repo', () => {
      const result = provider._parseIdentifier('1172', null);
      expect(result).to.deep.equal({ repo: null, number: '1172' });
    });
  });

  describe('never loses repo information', () => {
    it('explicit repo takes precedence over gitContext', () => {
      const gitContext = { owner: 'other', repo: 'repo' };
      const result = provider._parseIdentifier('covibes/covibes#1172', gitContext);
      expect(result.repo).to.equal('covibes/covibes');
    });

    it('URL repo takes precedence over gitContext', () => {
      const gitContext = { owner: 'other', repo: 'repo' };
      const result = provider._parseIdentifier(
        'https://github.com/covibes/covibes/issues/1172',
        gitContext
      );
      expect(result.repo).to.equal('covibes/covibes');
    });
  });
});
