const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'commands', 'dispatcher.js');

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { dispatchCommand } = require('../../lib/tui/commands/dispatcher');

function createContext() {
  const calls = {
    navigate: [],
    provider: null,
    exit: 0,
  };

  return {
    calls,
    context: {
      navigate: (view) => calls.navigate.push(view),
      setProvider: (provider) => {
        calls.provider = provider;
      },
      exit: () => {
        calls.exit += 1;
      },
    },
  };
}

describe('TUI command dispatcher', function () {
  it('handles /help', function () {
    const { context } = createContext();
    const result = dispatchCommand(
      { type: 'command', name: 'help', args: [], raw: '/help' },
      context
    );
    assert.strictEqual(result.tone, 'info');
    assert.ok(result.message.includes('/help'));
  });

  it('navigates on /monitor', function () {
    const { context, calls } = createContext();
    const result = dispatchCommand(
      { type: 'command', name: 'monitor', args: [], raw: '/monitor' },
      context
    );
    assert.strictEqual(result.tone, 'success');
    assert.deepStrictEqual(calls.navigate, ['monitor']);
  });

  it('stubs /issue', function () {
    const { context } = createContext();
    const result = dispatchCommand(
      { type: 'command', name: 'issue', args: ['123'], raw: '/issue 123' },
      context
    );
    assert.ok(result.message.toLowerCase().includes('not implemented'));
  });

  it('sets provider on /provider', function () {
    const { context, calls } = createContext();
    const result = dispatchCommand(
      { type: 'command', name: 'provider', args: ['codex'], raw: '/provider codex' },
      context
    );
    assert.strictEqual(result.tone, 'success');
    assert.strictEqual(calls.provider, 'codex');
  });

  it('rejects invalid providers', function () {
    const { context } = createContext();
    const result = dispatchCommand(
      {
        type: 'command',
        name: 'provider',
        args: ['invalid'],
        raw: '/provider invalid',
      },
      context
    );
    assert.strictEqual(result.tone, 'error');
  });

  it('exits on /quit', function () {
    const { context, calls } = createContext();
    const result = dispatchCommand(
      { type: 'command', name: 'quit', args: [], raw: '/quit' },
      context
    );
    assert.strictEqual(result.tone, 'info');
    assert.strictEqual(calls.exit, 1);
  });
});
