/**
 * Test: CLI Default Entry Behavior (no args)
 *
 * Verifies zeroshot with no args launches TUI on interactive TTY
 * and prints help on non-interactive input.
 */

const assert = require('assert');

function resolveDefaultEntry(args, { isInteractiveTty }) {
  let workingArgs = [...args];
  let shouldOutputHelp = false;

  if (workingArgs.length === 0) {
    if (isInteractiveTty) {
      workingArgs = ['tui'];
    } else {
      shouldOutputHelp = true;
    }
  }

  return { args: workingArgs, shouldOutputHelp };
}

describe('CLI Default Entry (no args)', function () {
  it('routes to tui when no args and interactive TTY', function () {
    const result = resolveDefaultEntry([], { isInteractiveTty: true });

    assert.deepStrictEqual(result.args, ['tui']);
    assert.strictEqual(result.shouldOutputHelp, false);
  });

  it('prints help when no args and non-interactive', function () {
    const result = resolveDefaultEntry([], { isInteractiveTty: false });

    assert.deepStrictEqual(result.args, []);
    assert.strictEqual(result.shouldOutputHelp, true);
  });

  it('does not change args when already provided', function () {
    const result = resolveDefaultEntry(['list'], { isInteractiveTty: true });

    assert.deepStrictEqual(result.args, ['list']);
    assert.strictEqual(result.shouldOutputHelp, false);
  });
});
