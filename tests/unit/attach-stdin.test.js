const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const { AttachServer } = require('../../src/attach');
const { sendInput } = require('../../src/attach/send-input');

describe('Attach stdin', function () {
  this.timeout(10000);

  it('sendInput writes to a live PTY', async function () {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-attach-'));
    const socketPath = path.join(tmpDir, 'attach.sock');

    const server = new AttachServer({
      id: 'attach-stdin-test',
      socketPath,
      command: 'cat',
      args: [],
      cwd: process.cwd(),
      env: process.env,
      cols: 80,
      rows: 24,
    });

    let output = '';
    let resolved = false;

    const outputPromise = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (!resolved) {
          resolved = true;
          reject(new Error('Timed out waiting for PTY output'));
        }
      }, 2000);

      const onOutput = (data) => {
        output += data.toString();
        if (output.includes('hello-attach')) {
          if (resolved) return;
          resolved = true;
          clearTimeout(timeout);
          server.off('output', onOutput);
          resolve();
        }
      };

      server.on('output', onOutput);
    });

    let testError;
    let stopError;
    try {
      await server.start();
      const result = await sendInput({
        socketPath,
        data: 'hello-attach\n',
        timeoutMs: 1000,
      });

      assert.strictEqual(result.ok, true);
      await outputPromise;
      assert.ok(output.includes('hello-attach'));
    } catch (error) {
      testError = error;
    } finally {
      try {
        await server.stop('SIGTERM');
      } catch (error) {
        console.warn('AttachServer.stop failed in attach-stdin test', error);
        stopError = error;
      }
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }

    if (stopError && !testError) {
      throw stopError;
    }
    if (testError) {
      throw testError;
    }
  });
});
