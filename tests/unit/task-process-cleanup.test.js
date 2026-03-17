const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const sinon = require('sinon');
const { pathToFileURL } = require('url');

function freshModule(modulePath) {
  return import(`${pathToFileURL(modulePath).href}?t=${Date.now()}-${Math.random()}`);
}

function buildTask(id, overrides = {}) {
  return {
    id,
    prompt: 'task prompt',
    fullPrompt: 'task prompt',
    cwd: process.cwd(),
    status: 'running',
    pid: 5678,
    watcherPid: 1234,
    sessionId: null,
    logFile: path.join(process.cwd(), `${id}.log`),
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    exitCode: null,
    error: null,
    provider: 'claude',
    model: null,
    scheduleId: null,
    socketPath: null,
    attachable: false,
    ...overrides,
  };
}

describe('task process cleanup', () => {
  let tempHome;
  let store;
  let runner;
  let killStub;

  beforeEach(async () => {
    tempHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-task-cleanup-'));
    process.env.ZEROSHOT_HOME = tempHome;
    store = await freshModule(path.join(__dirname, '..', '..', 'task-lib', 'store.js'));
    runner = await freshModule(path.join(__dirname, '..', '..', 'task-lib', 'runner.js'));
  });

  afterEach(() => {
    if (killStub) {
      killStub.restore();
      killStub = null;
    }
    delete process.env.ZEROSHOT_HOME;
    fs.rmSync(tempHome, { recursive: true, force: true });
  });

  it('persists watcherPid in the task store', () => {
    const task = buildTask('task-store-roundtrip');
    store.addTask(task);

    const saved = store.getTask(task.id);

    assert.strictEqual(saved.watcherPid, 1234);
    assert.strictEqual(saved.pid, 5678);
  });

  it('terminates the detached watcher group instead of only the child pid', async () => {
    const task = buildTask('task-terminate');
    const alive = { watcher: true, child: true };
    const calls = [];

    killStub = sinon.stub(process, 'kill').callsFake((pid, signal = 0) => {
      calls.push([pid, signal]);

      if (signal === 0) {
        if ((pid === 1234 && alive.watcher) || (pid === 5678 && alive.child)) {
          return true;
        }
        const error = new Error('missing');
        error.code = 'ESRCH';
        throw error;
      }

      if (pid === -1234 && signal === 'SIGTERM') {
        alive.watcher = false;
        alive.child = false;
        return true;
      }

      if (pid === 1234 || pid === 5678) {
        return true;
      }

      const error = new Error('missing');
      error.code = 'ESRCH';
      throw error;
    });

    const result = await runner.terminateTask(task, {
      timeoutMs: 100,
      forceKillTimeoutMs: 100,
    });

    assert.strictEqual(result.signaled, true);
    assert.strictEqual(result.exited, true);
    assert.strictEqual(result.forced, false);
    assert.ok(
      calls.some(([pid, signal]) => pid === -1234 && signal === 'SIGTERM'),
      'expected terminateTask() to signal the detached watcher process group'
    );
  });

  it('marks lost running tasks stale and clears both tracked pids', async () => {
    const task = buildTask('task-reconcile-stale');
    store.addTask(task);

    killStub = sinon.stub(process, 'kill').callsFake((_pid, signal = 0) => {
      if (signal === 0) {
        const error = new Error('missing');
        error.code = 'ESRCH';
        throw error;
      }
      return true;
    });

    const report = await runner.reconcileTasks();
    const updated = store.getTask(task.id);

    assert.ok(report.updated.includes(task.id));
    assert.strictEqual(updated.status, 'stale');
    assert.strictEqual(updated.pid, null);
    assert.strictEqual(updated.watcherPid, null);
    assert.strictEqual(updated.error, 'Process died unexpectedly');
  });
});
