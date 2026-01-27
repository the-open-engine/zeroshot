const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execSync } = require('child_process');
const { pathToFileURL } = require('url');

const buildOutput = path.join(__dirname, '..', '..', 'lib', 'tui', 'commands', 'dispatcher.js');

const originalHome = process.env.HOME;
const tempHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-tui-'));
process.env.HOME = tempHome;

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { dispatchCommand } = require('../../lib/tui/commands/dispatcher');

let seeded = false;

async function seedTasks() {
  if (seeded) return;
  const storeUrl = pathToFileURL(path.join(__dirname, '..', '..', 'task-lib', 'store.js')).href;
  const { saveTasks } = await import(storeUrl);
  const now = new Date().toISOString();
  saveTasks({
    'task-123': {
      id: 'task-123',
      prompt: 'Example prompt',
      fullPrompt: 'Example prompt',
      cwd: tempHome,
      status: 'completed',
      pid: null,
      sessionId: null,
      logFile: path.join(tempHome, 'task-123.log'),
      createdAt: now,
      updatedAt: now,
      exitCode: 0,
      error: null,
      provider: 'codex',
      model: 'test-model',
      scheduleId: null,
      socketPath: null,
      attachable: false,
    },
  });
  seeded = true;
}

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
  it('handles /help', async function () {
    const { context } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'help', args: [], raw: '/help' },
      context
    );
    assert.strictEqual(result.tone, 'info');
    assert.ok(result.message.includes('/help'));
  });

  it('navigates on /monitor', async function () {
    const { context, calls } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'monitor', args: [], raw: '/monitor' },
      context
    );
    assert.strictEqual(result.tone, 'success');
    assert.deepStrictEqual(calls.navigate, ['monitor']);
  });

  it('stubs /issue', async function () {
    const { context } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'issue', args: ['123'], raw: '/issue 123' },
      context
    );
    assert.ok(result.message.toLowerCase().includes('not implemented'));
  });

  it('sets provider on /provider', async function () {
    const { context, calls } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'provider', args: ['codex'], raw: '/provider codex' },
      context
    );
    assert.strictEqual(result.tone, 'success');
    assert.strictEqual(calls.provider, 'codex');
  });

  it('rejects invalid providers', async function () {
    const { context } = createContext();
    const result = await dispatchCommand(
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

  it('exits on /quit', async function () {
    const { context, calls } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'quit', args: [], raw: '/quit' },
      context
    );
    assert.strictEqual(result.tone, 'info');
    assert.strictEqual(calls.exit, 1);
  });

  it('handles /list', async function () {
    await seedTasks();
    const { context } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'list', args: [], raw: '/list' },
      context
    );
    assert.strictEqual(result.tone, 'info');
    assert.ok(result.message.includes('task-123'));
  });

  it('handles /status <id>', async function () {
    await seedTasks();
    const { context } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'status', args: ['task-123'], raw: '/status task-123' },
      context
    );
    assert.strictEqual(result.tone, 'info');
    assert.ok(result.message.includes('task-123'));
  });

  it('rejects /status without id', async function () {
    const { context } = createContext();
    const result = await dispatchCommand(
      { type: 'command', name: 'status', args: [], raw: '/status' },
      context
    );
    assert.strictEqual(result.tone, 'error');
    assert.ok(result.message.includes('Usage: /status <id>'));
  });
});

after(function () {
  process.env.HOME = originalHome;
});
