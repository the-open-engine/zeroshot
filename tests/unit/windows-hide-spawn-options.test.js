const assert = require('assert');
const fs = require('fs');
const path = require('path');
const sinon = require('sinon');
const { EventEmitter } = require('events');
const childProcess = require('child_process');

const projectRoot = path.resolve(__dirname, '..', '..');

function readSource(relativePath) {
  return fs.readFileSync(path.join(projectRoot, relativePath), 'utf8');
}

// Regression guard for GitHub issue #459: on Windows, spawned child
// processes must not pop up a visible console window. Node suppresses
// this only when `windowsHide: true` is passed explicitly; it is a
// no-op (safe) on non-Windows platforms.
describe('windowsHide spawn options (Windows console window fix)', function () {
  it('passes windowsHide: true when ClaudeTaskRunner spawns the zeroshot CLI', async function () {
    const fakeChild = new EventEmitter();
    fakeChild.stdout = new EventEmitter();
    fakeChild.stderr = new EventEmitter();

    const spawnStub = sinon.stub(childProcess, 'spawn').returns(fakeChild);
    try {
      // claude-task-runner.js destructures `spawn` from child_process at
      // require time, so the module must be (re)loaded after stubbing.
      const runnerPath = require.resolve('../../src/claude-task-runner');
      delete require.cache[runnerPath];
      const ClaudeTaskRunner = require(runnerPath);

      const runner = new ClaudeTaskRunner({ quiet: true });
      const pending = runner._spawnAndGetTaskId('zeroshot', ['task', 'run'], '/tmp', {}, 'agent-1');
      // spawn() is invoked synchronously inside the Promise executor.
      assert.strictEqual(spawnStub.calledOnce, true);
      const options = spawnStub.firstCall.args[2];
      assert.strictEqual(options.windowsHide, true);

      // Resolve the pending promise so it doesn't leak an unhandled rejection.
      fakeChild.emit('close', 1);
      await assert.rejects(pending);
    } finally {
      spawnStub.restore();
      // Force other tests/files to reload with the real (unstubbed) spawn.
      const runnerPath = require.resolve('../../src/claude-task-runner');
      delete require.cache[runnerPath];
    }
  });

  it('adds windowsHide: true to fork() options in spawnWatcher (task-lib/runner.js)', function () {
    const source = readSource('task-lib/runner.js');
    const match = source.match(/function spawnWatcher\([\s\S]*?\n\}/);
    assert(match, 'spawnWatcher() not found in task-lib/runner.js');
    assert.match(match[0], /fork\(/);
    assert.match(
      match[0],
      /windowsHide:\s*true/,
      'spawnWatcher() must pass windowsHide: true to fork()'
    );
  });

  it('adds windowsHide: true to the watcher child spawn (task-lib/watcher.js)', function () {
    const source = readSource('task-lib/watcher.js');
    const match = source.match(/const child = spawn\([\s\S]*?\);/);
    assert(match, 'watcher child spawn() call not found in task-lib/watcher.js');
    assert.match(
      match[0],
      /windowsHide:\s*true/,
      'watcher.js must pass windowsHide: true to spawn()'
    );
  });

  it('adds windowsHide: true to spawnTaskProcess (src/agent/agent-task-executor.js)', function () {
    const source = readSource('src/agent/agent-task-executor.js');
    const match = source.match(
      /function spawnTaskProcess\([\s\S]*?const proc = spawn\([\s\S]*?\);/
    );
    assert(match, 'spawnTaskProcess() spawn() call not found in agent-task-executor.js');
    assert.match(
      match[0],
      /windowsHide:\s*true/,
      'spawnTaskProcess() must pass windowsHide: true to spawn()'
    );
  });
});
