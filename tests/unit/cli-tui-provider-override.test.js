/**
 * Test: CLI TUI Provider Override
 *
 * Verifies that --provider parsing for `zeroshot tui` normalizes and validates
 * the provider override passed to the TUI bootstrap.
 */

const assert = require('assert');
const { buildRustTuiCommand, resolveTuiProviderOverride } = require('../../lib/tui-launcher');

describe('CLI TUI Provider Override', function () {
  it('returns null when no override is set', function () {
    const result = resolveTuiProviderOverride({});
    assert.strictEqual(result, null);
  });

  it('passes provider override into Rust TUI command', function () {
    const result = buildRustTuiCommand({ provider: 'codex', binaryPath: '/tmp/zeroshot-tui' });
    assert.deepStrictEqual(result.args, ['--provider-override', 'codex']);
    assert.strictEqual(result.env.ZEROSHOT_TUI_PROVIDER_OVERRIDE, 'codex');
  });

  it('normalizes provider aliases', function () {
    const result = resolveTuiProviderOverride({ provider: 'OpenAI' });
    assert.strictEqual(result, 'codex');
  });

  it('throws on unknown provider', function () {
    assert.throws(() => resolveTuiProviderOverride({ provider: 'invalid' }), /Unknown provider:/);
  });
});
