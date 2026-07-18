const assert = require('assert');
const { execFile, spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { URL } = require('url');
const { promisify } = require('util');

const execFileAsync = promisify(execFile);

describe('Task termination recovery', function () {
  this.timeout(10000);

  it('escalates graceful provider termination to a hard kill', async function () {
    const { terminateProcess } = await import('../task-lib/runner.js');
    const child = spawn(
      process.execPath,
      [path.resolve(__dirname, 'fixtures/sigterm-resistant-process.js')],
      { stdio: ['ignore', 'pipe', 'ignore'] }
    );
    await new Promise((resolve) => child.stdout.once('data', resolve));

    try {
      const result = await terminateProcess(child.pid, { graceMs: 40, pollMs: 5 });
      assert.strictEqual(result.terminated, true);
      assert.strictEqual(result.signal, 'SIGKILL');
      assert.strictEqual(result.escalated, true);
    } finally {
      if (child.exitCode === null) child.kill('SIGKILL');
    }
  });

  it('persists killed and already-dead tasks as terminal with no PID', async function () {
    const taskHome = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-task-kill-store-'));
    const storeUrl = new URL('../task-lib/store.js', `file://${__filename}`).href;
    const killUrl = new URL('../task-lib/commands/kill.js', `file://${__filename}`).href;
    const resistantScript = path.resolve(__dirname, 'fixtures/sigterm-resistant-process.js');
    const script = `
      import { spawn } from 'child_process';
      const { addTask, getTask } = await import(${JSON.stringify(storeUrl)});
      const { killTaskCommand } = await import(${JSON.stringify(killUrl)});
      const base = {
        prompt: 'hang', fullPrompt: 'hang', cwd: process.cwd(), status: 'running',
        sessionId: null, logFile: null, createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(), exitCode: null, error: null,
        provider: 'codex', model: 'fake', scheduleId: null, socketPath: null,
        attachable: false
      };
      const child = spawn(process.execPath, [${JSON.stringify(resistantScript)}],
        { stdio: ['ignore', 'pipe', 'ignore'] });
      await new Promise((resolve) => child.stdout.once('data', resolve));
      addTask({ ...base, id: 'live-task', pid: child.pid });
      await killTaskCommand('live-task', { graceMs: 40, pollMs: 5 });
      addTask({ ...base, id: 'dead-task', pid: 99999999 });
      await killTaskCommand('dead-task', { graceMs: 40, pollMs: 5 });
      console.log('RESULT:' + JSON.stringify({
        live: getTask('live-task'),
        dead: getTask('dead-task')
      }));
    `;

    try {
      const { stdout } = await execFileAsync(
        process.execPath,
        ['--input-type=module', '-e', script],
        {
          env: {
            ...process.env,
            HOME: taskHome,
            USERPROFILE: taskHome,
            ZEROSHOT_HOME: taskHome,
          },
        }
      );
      const line = stdout.split('\n').find((entry) => entry.startsWith('RESULT:'));
      const terminal = JSON.parse(line.slice('RESULT:'.length));
      assert.deepStrictEqual(
        [terminal.live.status, terminal.live.pid, terminal.dead.status, terminal.dead.pid],
        ['killed', null, 'stale', null]
      );
      assert.match(terminal.live.error, /SIGKILL/);
    } finally {
      fs.rmSync(taskHome, { recursive: true, force: true });
    }
  });
});
