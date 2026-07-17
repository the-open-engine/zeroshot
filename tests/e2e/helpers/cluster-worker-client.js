'use strict';

const { spawn } = require('node:child_process');

function openClusterWorker({ fixture, cwd, env, timeoutMs = 1000 }) {
  const child = spawn(process.execPath, [fixture], {
    cwd,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const waiters = new Map();
  let stdout = '';
  let stderr = '';

  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    let newline;
    while ((newline = stdout.indexOf('\n')) !== -1) {
      const frame = JSON.parse(stdout.slice(0, newline));
      stdout = stdout.slice(newline + 1);
      if (frame.type === 'response') waiters.get(String(frame.id))?.(frame);
    }
  });

  function send(id, method, params = {}) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`Timed out waiting for ${id}: ${stderr}`)),
        timeoutMs
      );
      waiters.set(String(id), (frame) => {
        clearTimeout(timer);
        waiters.delete(String(id));
        resolve(frame);
      });
      child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
    });
  }

  async function close() {
    child.stdin.end();
    await new Promise((resolve, reject) => {
      child.once('exit', resolve);
      child.once('error', reject);
    });
  }

  return { child, close, send };
}

module.exports = { openClusterWorker };
