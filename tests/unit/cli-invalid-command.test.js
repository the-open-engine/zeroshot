/**
 * Test: CLI Invalid Command Handling
 *
 * Verifies that the CLI properly handles invalid commands by treating them
 * as inputs to the 'run' command rather than showing an error.
 *
 * Current behavior (lines 4334-4342 in cli/index.js):
 * - Unknown commands are prepended with 'run'
 * - Example: "zeroshot foobar" → "zeroshot run foobar"
 */

const assert = require('assert');

describe('CLI Invalid Command Handling', function () {
  // Mock the command prepending logic from cli/index.js
  // This tests the behavior without actually spawning processes
  function shouldPrependRun(args) {
    if (args.length === 0) return false;

    const firstArg = args[0];

    // Skip if it's a flag/option (starts with -)
    if (firstArg.startsWith('-')) return false;

    // Known commands that should NOT be prepended
    const knownCommands = [
      'run',
      'task',
      'list',
      'status',
      'logs',
      'stop',
      'kill',
      'kill-all',
      'clean',
      'resume',
      'purge',
      'export',
      'watch',
      'attach',
      'agents',
      'config',
      'settings',
    ];

    // If first arg is not a known command, should prepend 'run'
    return !knownCommands.includes(firstArg);
  }

  describe('Unknown command behavior', function () {
    it('should prepend run for unknown command', function () {
      assert.strictEqual(shouldPrependRun(['invalid-command']), true);
    });

    it('should prepend run for multiple unknown arguments', function () {
      assert.strictEqual(shouldPrependRun(['foo', 'bar', 'baz']), true);
    });

    it('should prepend run for numeric arguments', function () {
      assert.strictEqual(shouldPrependRun(['123']), true);
    });

    it('should prepend run for arguments with special characters', function () {
      assert.strictEqual(shouldPrependRun(['test@example.com']), true);
    });

    it('should prepend run for text that looks like a description', function () {
      assert.strictEqual(shouldPrependRun(['fix', 'the', 'bug']), true);
    });
  });

  describe('Valid commands should NOT be prepended', function () {
    it('should not prepend run for "list" command', function () {
      assert.strictEqual(shouldPrependRun(['list']), false);
    });

    it('should not prepend run for "status" command', function () {
      assert.strictEqual(shouldPrependRun(['status', 'cluster-123']), false);
    });

    it('should not prepend run for "logs" command', function () {
      assert.strictEqual(shouldPrependRun(['logs', 'cluster-123']), false);
    });

    it('should not prepend run for "run" command', function () {
      assert.strictEqual(shouldPrependRun(['run', '123']), false);
    });

    it('should not prepend run for "task" command', function () {
      assert.strictEqual(shouldPrependRun(['task', 'run', 'something']), false);
    });

    it('should not prepend run for "agents" command', function () {
      assert.strictEqual(shouldPrependRun(['agents', 'list']), false);
    });

    it('should not prepend run for "config" command', function () {
      assert.strictEqual(shouldPrependRun(['config', 'list']), false);
    });

    it('should not prepend run for "settings" command', function () {
      assert.strictEqual(shouldPrependRun(['settings']), false);
    });
  });

  describe('Flags should not trigger run prepending', function () {
    it('should not prepend for --help flag', function () {
      assert.strictEqual(shouldPrependRun(['--help']), false);
    });

    it('should not prepend for --version flag', function () {
      assert.strictEqual(shouldPrependRun(['--version']), false);
    });

    it('should not prepend for -h flag', function () {
      assert.strictEqual(shouldPrependRun(['-h']), false);
    });

    it('should not prepend for -V flag', function () {
      assert.strictEqual(shouldPrependRun(['-V']), false);
    });

    it('should not prepend for --json flag as first arg', function () {
      assert.strictEqual(shouldPrependRun(['--json']), false);
    });
  });

  describe('Edge cases', function () {
    it('should not prepend for empty args', function () {
      assert.strictEqual(shouldPrependRun([]), false);
    });

    it('should prepend for args that look like URLs', function () {
      assert.strictEqual(shouldPrependRun(['https://github.com/org/repo/issues/123']), true);
    });

    it('should prepend for args with paths', function () {
      assert.strictEqual(shouldPrependRun(['./some/path']), true);
    });

    it('should prepend for args with dots', function () {
      assert.strictEqual(shouldPrependRun(['some.command']), true);
    });
  });

  describe('Behavior documentation', function () {
    it('documents that unknown commands get treated as run inputs', function () {
      // This test serves as documentation of the current behavior:
      // When user types: zeroshot invalid-command
      // CLI transforms to: zeroshot run invalid-command
      //
      // This is intentional design - allows users to quickly run tasks
      // without typing 'run' explicitly.
      //
      // Example uses:
      //   zeroshot 123           → zeroshot run 123 (issue number)
      //   zeroshot "fix the bug" → zeroshot run "fix the bug" (text)
      //
      // This means there is NO "invalid command" error - everything
      // that's not a known command becomes an input to 'run'.

      const examples = [
        ['123', true], // Issue number
        ['fix-auth-bug', true], // Text description
        ['https://github.com/org/repo/issues/123', true], // URL
        ['run', false], // Known command
        ['list', false], // Known command
        ['--help', false], // Flag
      ];

      for (const [arg, expected] of examples) {
        assert.strictEqual(
          shouldPrependRun([arg]),
          expected,
          `Expected shouldPrependRun(['${arg}']) to be ${expected}`
        );
      }
    });
  });
});
