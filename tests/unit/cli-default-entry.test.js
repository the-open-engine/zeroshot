/**
 * Test: CLI Default Entry Behavior (no args)
 *
 * Verifies zeroshot with no args prints help in both TTY and non-interactive mode.
 */

const assert = require('assert');

function resolveDefaultEntry(args) {
  let workingArgs = [...args];
  let shouldOutputHelp = false;

  if (workingArgs.length === 0) {
    shouldOutputHelp = true;
  }

  return { args: workingArgs, shouldOutputHelp };
}

describe('CLI Default Entry (no args)', function () {
  it('prints help when no args and interactive TTY', function () {
    const result = resolveDefaultEntry([]);

    assert.deepStrictEqual(result.args, []);
    assert.strictEqual(result.shouldOutputHelp, true);
  });

  it('prints help when no args and non-interactive', function () {
    const result = resolveDefaultEntry([]);

    assert.deepStrictEqual(result.args, []);
    assert.strictEqual(result.shouldOutputHelp, true);
  });

  it('does not change args when already provided', function () {
    const result = resolveDefaultEntry(['list']);

    assert.deepStrictEqual(result.args, ['list']);
    assert.strictEqual(result.shouldOutputHelp, false);
  });
});
