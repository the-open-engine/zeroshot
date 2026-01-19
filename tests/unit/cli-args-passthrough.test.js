/**
 * Test: CLI Args Passthrough
 *
 * Verifies that --cli-args option correctly passes extra flags to Claude CLI.
 * This enables features like --chrome for browser control, --verbose, etc.
 */

const assert = require('assert');

/**
 * Simulates the args building logic from task-lib/runner.js
 * This mirrors the logic that injects cliArgs into the claude command.
 */
function buildClaudeArgs(options) {
  const outputFormat = options.outputFormat || 'stream-json';
  const args = ['--print', '--dangerously-skip-permissions', '--output-format', outputFormat];

  // Add any extra CLI args passed through (e.g., "--chrome" for browser control)
  if (options.cliArgs) {
    const extraArgs = options.cliArgs.split(/\s+/).filter(arg => arg.length > 0);
    args.push(...extraArgs);
  }

  // Only add streaming options for stream-json format
  if (outputFormat === 'stream-json') {
    args.push('--verbose');
    args.push('--include-partial-messages');
  }

  return args;
}

describe('CLI Args Passthrough', function () {
  describe('--cli-args option', function () {
    it('should pass through single flag (--chrome)', function () {
      const args = buildClaudeArgs({ cliArgs: '--chrome' });

      assert.ok(args.includes('--chrome'), 'Should include --chrome flag');
    });

    it('should pass through multiple flags', function () {
      const args = buildClaudeArgs({ cliArgs: '--chrome --verbose --debug' });

      assert.ok(args.includes('--chrome'), 'Should include --chrome');
      // Note: --verbose from cliArgs will appear, plus --verbose from stream-json format
      assert.ok(args.includes('--debug'), 'Should include --debug');
    });

    it('should handle extra whitespace in cliArgs', function () {
      const args = buildClaudeArgs({ cliArgs: '  --chrome   --debug  ' });

      assert.ok(args.includes('--chrome'), 'Should include --chrome');
      assert.ok(args.includes('--debug'), 'Should include --debug');
      // Empty strings should be filtered out
      assert.ok(!args.includes(''), 'Should not include empty strings');
    });

    it('should work with empty cliArgs', function () {
      const args = buildClaudeArgs({ cliArgs: '' });

      // Should still have base args
      assert.ok(args.includes('--print'), 'Should include --print');
      assert.ok(args.includes('--dangerously-skip-permissions'), 'Should include permissions flag');
    });

    it('should work without cliArgs option', function () {
      const args = buildClaudeArgs({});

      // Should still have base args
      assert.ok(args.includes('--print'), 'Should include --print');
      assert.ok(args.includes('--dangerously-skip-permissions'), 'Should include permissions flag');
      assert.ok(args.includes('--verbose'), 'Should include --verbose for stream-json');
    });

    it('should preserve order: base args, then cliArgs, then format-specific', function () {
      const args = buildClaudeArgs({ cliArgs: '--chrome' });

      const printIndex = args.indexOf('--print');
      const chromeIndex = args.indexOf('--chrome');
      const verboseIndex = args.indexOf('--verbose');

      // Base args should come first
      assert.ok(printIndex < chromeIndex, '--print should come before --chrome');
      // cliArgs should come before stream-json specific options
      assert.ok(chromeIndex < verboseIndex, '--chrome should come before --verbose');
    });
  });

  describe('Output format interaction', function () {
    it('should work with text output format', function () {
      const args = buildClaudeArgs({ cliArgs: '--chrome', outputFormat: 'text' });

      assert.ok(args.includes('--chrome'), 'Should include --chrome');
      assert.ok(args.includes('--output-format'), 'Should include --output-format');
      assert.ok(args.includes('text'), 'Should include text format');
      // text format should NOT have streaming options
      assert.ok(!args.includes('--include-partial-messages'), 'Should not have streaming options');
    });

    it('should work with json output format', function () {
      const args = buildClaudeArgs({ cliArgs: '--chrome', outputFormat: 'json' });

      assert.ok(args.includes('--chrome'), 'Should include --chrome');
      assert.ok(args.includes('json'), 'Should include json format');
    });
  });
});
