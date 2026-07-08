/**
 * Regression test for issue #575: `zeroshot run <linear-url>` was treated as
 * manual text because detectRunInput() maintained its own private copy of
 * provider URL/key regexes (no linear.app pattern) instead of delegating to
 * the provider registry. detectRunInput() now delegates to detectProvider()
 * so there is a single source of truth for "is this input a recognized issue".
 */

const assert = require('assert');

const {
  detectRunInput,
  buildIssueInput,
  buildTextInput,
  buildFileInput,
} = require('../../lib/start-cluster');
const { registerProvider, listProviders } = require('../../src/issue-providers');
const IssueProvider = require('../../src/issue-providers/base-provider');

describe('detectRunInput()', function () {
  describe('Linear routing (issue #575)', function () {
    const linearUrl = 'https://linear.app/acme/issue/THE-5/some-title';

    it('routes a linear.app issue URL to buildIssueInput, not buildTextInput', function () {
      const result = detectRunInput(linearUrl, {}, null);
      assert.deepStrictEqual(result, buildIssueInput(linearUrl));
      assert.strictEqual(result.text, undefined);
    });

    it('routes a linear.app issue URL to buildIssueInput when forceProvider="linear"', function () {
      const result = detectRunInput(linearUrl, {}, 'linear');
      assert.deepStrictEqual(result, buildIssueInput(linearUrl));
    });
  });

  describe('existing per-provider behavior is preserved', function () {
    it('GitHub issue URL routes to issue input', function () {
      const url = 'https://github.com/owner/repo/issues/123';
      assert.deepStrictEqual(detectRunInput(url, {}, null), buildIssueInput(url));
    });

    it('GitHub bare number (no defaultIssueSource set) routes to issue input', function () {
      // Resolved via git-context detection (this repo's origin is GitHub) or,
      // absent that, the GitHub legacy fallback in base-provider.js.
      assert.deepStrictEqual(detectRunInput('123', {}, null), buildIssueInput('123'));
    });

    it('GitLab issue URL routes to issue input', function () {
      const url = 'https://gitlab.com/owner/repo/-/issues/456';
      assert.deepStrictEqual(detectRunInput(url, {}, null), buildIssueInput(url));
    });

    it('Jira Cloud issue URL routes to issue input', function () {
      const url = 'https://acme.atlassian.net/browse/ENG-42';
      assert.deepStrictEqual(detectRunInput(url, {}, null), buildIssueInput(url));
    });

    it('Jira issue key routes to issue input', function () {
      assert.deepStrictEqual(detectRunInput('ENG-42', {}, null), buildIssueInput('ENG-42'));
    });

    it('Azure DevOps work item URL routes to issue input', function () {
      const url = 'https://dev.azure.com/org/project/_workitems/edit/789';
      assert.deepStrictEqual(detectRunInput(url, {}, null), buildIssueInput(url));
    });

    it('markdown file routes to file input', function () {
      assert.deepStrictEqual(detectRunInput('feature.md', {}, null), buildFileInput('feature.md'));
    });

    it('plain text routes to text input', function () {
      const text = 'Implement dark mode toggle';
      assert.deepStrictEqual(detectRunInput(text, {}, null), buildTextInput(text));
    });
  });

  describe('regression guard: detectRunInput stays in sync with the provider registry', function () {
    it('recognizes a newly registered provider with zero changes to detectRunInput', function () {
      class FakeProvider extends IssueProvider {
        static id = 'fake-test-provider-575';
        static displayName = 'Fake Test Provider';
        static detectIdentifier(input) {
          return input === 'FAKE-MARKER-575';
        }
      }
      registerProvider(FakeProvider);

      assert.ok(listProviders().includes(FakeProvider.id));
      assert.deepStrictEqual(
        detectRunInput('FAKE-MARKER-575', {}, null),
        buildIssueInput('FAKE-MARKER-575')
      );
    });
  });
});
