'use strict';

// CI smoke for the embedded application; no development server or provider calls.
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { setTimeout: delay } = require('node:timers/promises');

async function within(promise, milliseconds) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error('UI smoke timed out')), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function get(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(3000) });
  assert.equal(response.status, 200, `${url}: HTTP ${response.status}`);
  return response;
}

async function smoke(url) {
  const base = new URL('/ui/', url);
  const deadline = Date.now() + 20_000;
  let index;
  while (!index) {
    try {
      index = await get(base);
    } catch (error) {
      if (Date.now() >= deadline) throw error;
      await delay(200);
    }
  }
  assert.match(index.headers.get('content-type'), /text\/html/);
  const html = await index.text();
  assert.match(html, /id="root"/);
  const assets = [...html.matchAll(/(?:src|href)="([^"]+\.(?:js|css))"/g)];
  assert.ok(assets.length > 0, 'application must reference built assets');
  for (const [, asset] of assets) {
    const address = new URL(asset, base);
    assert.equal(address.origin, base.origin, 'assets must be bundled with the server');
    const response = await get(address);
    assert.doesNotMatch(response.headers.get('content-type'), /text\/html/);
    assert.ok((await response.arrayBuffer()).byteLength > 0);
  }
  for (const endpoint of ['bootstrap', 'profiles', 'runs']) {
    const response = await get(new URL(`api/${endpoint}`, base));
    assert.match(response.headers.get('content-type'), /application\/json/);
    const value = await response.json();
    assert.ok(value && typeof value === 'object', endpoint);
  }
}

async function local(binary) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'zeroshot-ui-smoke-'));
  const child = spawn(path.resolve(binary), ['ui', '--listen', '127.0.0.1:0'], {
    env: {
      ...process.env,
      ZEROSHOT_CONFIG_DIR: path.join(directory, 'config'),
      ZEROSHOT_STATE_DIR: path.join(directory, 'state'),
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  let settled = false;
  const capture = (chunk) => {
    output = (output + chunk).slice(-65536);
  };
  child.stdout.on('data', capture);
  child.stderr.on('data', capture);
  const exited = new Promise((resolve) => {
    child.once('error', (error) => {
      settled = true;
      resolve({ error });
    });
    child.once('exit', (code, signal) => {
      settled = true;
      resolve({ code, signal });
    });
  });
  try {
    const address = await within(
      (async () => {
        for (;;) {
          const match = output.match(/http:\/\/127\.0\.0\.1:\d+\/ui\//);
          if (match) return match[0];
          assert.equal(settled, false, `UI exited before listening: ${output}`);
          await delay(50);
        }
      })(),
      10_000
    );
    await smoke(address);
    child.kill('SIGTERM');
    const result = await within(exited, 20_000);
    assert.deepEqual(
      result,
      process.platform === 'win32' ? { code: null, signal: 'SIGTERM' } : { code: 0, signal: null },
      output
    );
  } finally {
    if (!settled) {
      child.kill('SIGKILL');
      await within(exited, 5000);
    }
    await fs.rm(directory, { recursive: true, force: true });
  }
}

async function main() {
  const [mode, value, ...extra] = process.argv.slice(2);
  assert.ok(value && extra.length === 0, 'usage: smoke-ui.js --binary PATH | --url ORIGIN');
  if (mode === '--binary') await local(value);
  else if (mode === '--url') await smoke(value);
  else throw new Error(`unknown smoke mode: ${mode}`);
  process.stdout.write('Embedded UI smoke passed\n');
}

main().catch((error) => {
  process.stderr.write(`${error.stack}\n`);
  process.exitCode = 1;
});
