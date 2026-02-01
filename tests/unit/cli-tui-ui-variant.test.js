/**
 * Test: CLI TUI UI Variant
 *
 * Verifies UI variant parsing and Rust TUI args/env plumbing.
 */

const assert = require('assert');
const { buildRustTuiCommand, resolveUiVariant } = require('../../lib/tui-launcher');

describe('CLI TUI UI Variant', function () {
  it('returns null when no variant is set', function () {
    const result = resolveUiVariant({});
    assert.strictEqual(result, null);
  });

  it('passes normalized ui variant into Rust TUI command', function () {
    const result = buildRustTuiCommand({ ui: 'Disruptive', binaryPath: '/tmp/zeroshot-tui' });
    assert.deepStrictEqual(result.args, ['--ui', 'disruptive']);
    assert.strictEqual(result.env.ZEROSHOT_TUI_UI, 'disruptive');
  });

  it('throws on unknown ui variant', function () {
    assert.throws(() => resolveUiVariant({ ui: 'weird' }), /Unknown UI variant/);
  });
});
