/**
 * Test: CLI Input Detection
 *
 * Verifies the input detection logic in cli/index.js
 * Tests: GitHub issue URL, issue number, org/repo#123, markdown files, plain text
 */

const assert = require('assert');

// Mock the CLI input detection logic
// This mirrors the logic in cli/index.js lines 497-516
function detectInputType(inputArg) {
  const input = {};

  // Check if it's a GitHub issue URL
  if (inputArg.match(/^https?:\/\/github\.com\/[\w-]+\/[\w-]+\/issues\/\d+/)) {
    input.issue = inputArg;
  }
  // Check if it's a GitHub issue number (just digits)
  else if (/^\d+$/.test(inputArg)) {
    input.issue = inputArg;
  }
  // Check if it's org/repo#123 format
  else if (inputArg.match(/^[\w-]+\/[\w-]+#\d+$/)) {
    input.issue = inputArg;
  }
  // Check if it's a markdown file (.md or .markdown)
  else if (/\.(md|markdown)$/i.test(inputArg)) {
    input.file = inputArg;
  }
  // Otherwise, treat as plain text
  else {
    input.text = inputArg;
  }

  return input;
}

describe('CLI Input Detection', function () {
  describe('GitHub issue detection', function () {
    it('should detect GitHub issue URL', function () {
      const input = detectInputType('https://github.com/owner/repo/issues/123');

      assert.strictEqual(input.issue, 'https://github.com/owner/repo/issues/123');
      assert.strictEqual(input.file, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect GitHub issue number', function () {
      const input = detectInputType('123');

      assert.strictEqual(input.issue, '123');
      assert.strictEqual(input.file, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect org/repo#123 format', function () {
      const input = detectInputType('owner/repo#456');

      assert.strictEqual(input.issue, 'owner/repo#456');
      assert.strictEqual(input.file, undefined);
      assert.strictEqual(input.text, undefined);
    });
  });

  describe('Markdown file detection', function () {
    it('should detect .md file', function () {
      const input = detectInputType('feature.md');

      assert.strictEqual(input.file, 'feature.md');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect .markdown file', function () {
      const input = detectInputType('feature.markdown');

      assert.strictEqual(input.file, 'feature.markdown');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect .MD file (uppercase)', function () {
      const input = detectInputType('README.MD');

      assert.strictEqual(input.file, 'README.MD');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect relative path to markdown file', function () {
      const input = detectInputType('./docs/feature.md');

      assert.strictEqual(input.file, './docs/feature.md');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect absolute path to markdown file', function () {
      const input = detectInputType('/tmp/feature.md');

      assert.strictEqual(input.file, '/tmp/feature.md');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('should detect parent directory path to markdown file', function () {
      const input = detectInputType('../feature.markdown');

      assert.strictEqual(input.file, '../feature.markdown');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });
  });

  describe('Plain text detection', function () {
    it('should treat plain text as text input', function () {
      const input = detectInputType('Implement dark mode');

      assert.strictEqual(input.text, 'Implement dark mode');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.file, undefined);
    });

    it('should treat sentence with spaces as text input', function () {
      const input = detectInputType('Add user authentication to the app');

      assert.strictEqual(input.text, 'Add user authentication to the app');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.file, undefined);
    });
  });

  describe('Edge cases', function () {
    it('file named "123.md" should be detected as file, not issue', function () {
      const input = detectInputType('123.md');

      // .md extension detection runs AFTER digit-only check
      // So this should be a file, not issue #123
      assert.strictEqual(input.file, '123.md');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });

    it('text containing "issue" should be plain text', function () {
      const input = detectInputType('Fix the issue with login');

      assert.strictEqual(input.text, 'Fix the issue with login');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.file, undefined);
    });

    it('markdown file with spaces in path', function () {
      const input = detectInputType('./docs/Feature Request.md');

      assert.strictEqual(input.file, './docs/Feature Request.md');
      assert.strictEqual(input.issue, undefined);
      assert.strictEqual(input.text, undefined);
    });
  });

  describe('Priority order', function () {
    it('GitHub URL has highest priority', function () {
      // Even if URL contains ".md", it's still a GitHub URL
      const input = detectInputType('https://github.com/owner/repo/issues/123');

      assert.strictEqual(input.issue, 'https://github.com/owner/repo/issues/123');
    });

    it('Issue number has priority over text', function () {
      const input = detectInputType('42');

      assert.strictEqual(input.issue, '42');
      assert.strictEqual(input.text, undefined);
    });

    it('File extension has priority over plain text', function () {
      const input = detectInputType('feature.md');

      assert.strictEqual(input.file, 'feature.md');
      assert.strictEqual(input.text, undefined);
    });
  });
});
