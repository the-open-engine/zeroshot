/**
 * Test: CLI TUI Launcher
 *
 * Verifies Rust TUI spawn is default and Ink fallback respects ZEROSHOT_TUI=ink.
 */

const assert = require('assert');
const { launchTuiSession } = require('../../lib/tui-launcher');

describe('CLI TUI Launcher', function () {
  const originalEnv = process.env.ZEROSHOT_TUI;

  afterEach(function () {
    if (originalEnv === undefined) {
      delete process.env.ZEROSHOT_TUI;
    } else {
      process.env.ZEROSHOT_TUI = originalEnv;
    }
  });

  it('spawns Rust TUI by default with initial screen + provider override', function () {
    delete process.env.ZEROSHOT_TUI;
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

  it('uses Ink fallback when ZEROSHOT_TUI=ink', function () {
    process.env.ZEROSHOT_TUI = 'ink';
    let inkOptions = null;
    let spawnCalled = false;

    const startInk = (options) => {
      inkOptions = options;
    };

    const spawnStub = () => {
      spawnCalled = true;
    };

    launchTuiSession({
      initialView: 'monitor',
      provider: 'claude',
      startInk,
      spawn: spawnStub,
    });

    assert.strictEqual(spawnCalled, false);
    assert.deepStrictEqual(inkOptions, {
      autoExit: false,
      providerOverride: 'claude',
      initialView: 'monitor',
    });
  });
});
