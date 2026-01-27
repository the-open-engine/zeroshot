const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'commands', 'registry.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { createCommandRegistry } = require('../../lib/tui/commands/registry');

function createContext() {
  return {
    navigate: () => {},
    setProvider: () => {},
    exit: () => {},
  };
}

describe('TUI command registry', function () {
  it('registers and dispatches commands', async function () {
    const registry = createCommandRegistry();
    registry.register({
      name: 'ping',
      description: 'ping',
      handler: () => ({ tone: 'success', message: 'pong' }),
    });

    const result = await registry.dispatch(
      { type: 'command', name: 'ping', args: [], raw: '/ping' },
      createContext()
    );

    assert.strictEqual(result.tone, 'success');
    assert.strictEqual(result.message, 'pong');
  });

  it('lists registered commands', function () {
    const registry = createCommandRegistry();
    registry.register({
      name: 'alpha',
      description: 'alpha',
      handler: () => ({ tone: 'info', message: 'alpha' }),
    });
    registry.register({
      name: 'beta',
      description: 'beta',
      handler: () => ({ tone: 'info', message: 'beta' }),
    });

    const names = registry.list().map((command) => command.name);
    assert.deepStrictEqual(names, ['alpha', 'beta']);
  });

  it('returns unknown command error', async function () {
    const registry = createCommandRegistry();
    const result = await registry.dispatch(
      { type: 'command', name: 'missing', args: [], raw: '/missing' },
      createContext()
    );

    assert.strictEqual(result.tone, 'error');
    assert.ok(result.message.includes('/missing'));
  });
});
