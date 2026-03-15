const assert = require('assert');
const { parseShortStat, formatRunDiffSummary } = require('../../cli/lib/run-diff-summary');

describe('run-diff-summary', function () {
  describe('parseShortStat', function () {
    it('should parse standard git diff --shortstat output', function () {
      const input = ' 3 files changed, 42 insertions(+), 7 deletions(-)';
      const result = parseShortStat(input);

      assert.strictEqual(result.filesChanged, 3);
      assert.strictEqual(result.insertions, 42);
      assert.strictEqual(result.deletions, 7);
    });

    it('should handle single file changed', function () {
      const input = ' 1 file changed, 10 insertions(+)';
      const result = parseShortStat(input);

      assert.strictEqual(result.filesChanged, 1);
      assert.strictEqual(result.insertions, 10);
      assert.strictEqual(result.deletions, 0);
    });

    it('should handle only deletions', function () {
      const input = ' 2 files changed, 15 deletions(-)';
      const result = parseShortStat(input);

      assert.strictEqual(result.filesChanged, 2);
      assert.strictEqual(result.insertions, 0);
      assert.strictEqual(result.deletions, 15);
    });

    it('should handle empty string', function () {
      const result = parseShortStat('');

      assert.strictEqual(result.filesChanged, 0);
      assert.strictEqual(result.insertions, 0);
      assert.strictEqual(result.deletions, 0);
    });
  });

  describe('formatRunDiffSummary', function () {
    it('should format valid diff', function () {
      const diff = {
        available: true,
        commits: 3,
        filesChanged: 5,
        insertions: 42,
        deletions: 7,
      };

      const result = formatRunDiffSummary(diff);
      assert.strictEqual(result, '3c 5f +42/-7');
    });

    it('should return dash for unavailable diff', function () {
      const diff = {
        available: false,
        reason: 'no_git_context',
      };

      const result = formatRunDiffSummary(diff);
      assert.strictEqual(result, '-');
    });

    it('should return dash for null diff', function () {
      const result = formatRunDiffSummary(null);
      assert.strictEqual(result, '-');
    });
  });
});
