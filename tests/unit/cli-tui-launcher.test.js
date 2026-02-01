/**
 * Test: CLI TUI Launcher
 *
 * Verifies Rust TUI spawn is default.
 */

const assert = require('assert');
const { launchTuiSession } = require('../../lib/tui-launcher');

describe('CLI TUI Launcher', function () {
  it('spawns Rust TUI by default with initial screen + provider override', function () {
    const spawnCalls = [];
    const spawnStub = (command, args, options) => {
      spawnCalls.push({ command, args, options });
      return { on: () => {} };
    };

    launchTuiSession({
      initialView: 'monitor',
      provider: 'codex',
      spawn: spawnStub,
      binaryPath: '/tmp/zeroshot-tui',
      cwd: '/tmp',
    });

    assert.strictEqual(spawnCalls.length, 1);
    assert.strictEqual(spawnCalls[0].command, '/tmp/zeroshot-tui');
    assert.deepStrictEqual(spawnCalls[0].args, [
      '--initial-screen',
      'monitor',
      '--provider-override',
      'codex',
    ]);
    assert.strictEqual(spawnCalls[0].options.cwd, '/tmp');
    assert.strictEqual(spawnCalls[0].options.stdio, 'inherit');
    assert.strictEqual(spawnCalls[0].options.env.ZEROSHOT_TUI_INITIAL_SCREEN, 'monitor');
    assert.strictEqual(spawnCalls[0].options.env.ZEROSHOT_TUI_PROVIDER_OVERRIDE, 'codex');
  });
});
