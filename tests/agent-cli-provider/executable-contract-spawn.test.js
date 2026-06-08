const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

test('spawnProcessRunner strips inherited process-control env before spawn', async () => {
  const { spawnProcessRunner } = require('../../lib/agent-cli-provider');
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-provider-env-'));
  const preloadPath = path.join(tempDir, 'preload.cjs');
  const markerPath = path.join(tempDir, 'marker');
  const previousNodeOptions = process.env.NODE_OPTIONS;

  fs.writeFileSync(
    preloadPath,
    `require('node:fs').writeFileSync(${JSON.stringify(markerPath)}, 'pwned');\n`
  );
  process.env.NODE_OPTIONS = `--require ${preloadPath}`;

  try {
    const result = await spawnProcessRunner()({
      binary: process.execPath,
      args: ['-e', 'process.stdout.write("ok")'],
      env: {},
      cleanupMetadata: [],
      warnings: [],
      redactions: [],
    });

    assert.equal(result.exitCode, 0);
    assert.equal(result.stdout, 'ok');
    assert.equal(fs.existsSync(markerPath), false);
  } finally {
    if (previousNodeOptions === undefined) {
      delete process.env.NODE_OPTIONS;
    } else {
      process.env.NODE_OPTIONS = previousNodeOptions;
    }
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test('spawnProcessRunner strips command spec process-control env before spawn', async () => {
  const { spawnProcessRunner } = require('../../lib/agent-cli-provider');
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-provider-env-'));
  const preloadPath = path.join(tempDir, 'preload.cjs');
  const markerPath = path.join(tempDir, 'marker');

  fs.writeFileSync(
    preloadPath,
    `require('node:fs').writeFileSync(${JSON.stringify(markerPath)}, 'pwned');\n`
  );

  try {
    const result = await spawnProcessRunner()({
      binary: process.execPath,
      args: ['-e', 'process.stdout.write("ok")'],
      env: {
        NODE_OPTIONS: `--require ${preloadPath}`,
      },
      cleanupMetadata: [],
      warnings: [],
      redactions: [],
    });

    assert.equal(result.exitCode, 0);
    assert.equal(result.stdout, 'ok');
    assert.equal(fs.existsSync(markerPath), false);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test('spawnProcessRunner escalates timed-out providers and reports timeout evidence', async () => {
  const { spawnProcessRunner } = require('../../lib/agent-cli-provider');
  const startedAt = Date.now();

  const result = await spawnProcessRunner()(
    {
      binary: process.execPath,
      args: ['-e', 'process.on("SIGTERM",()=>{}); setTimeout(()=>process.exit(0),800);'],
      env: {},
      cleanupMetadata: [],
      warnings: [],
      redactions: [],
    },
    { timeoutMs: 50 }
  );

  assert.equal(result.timedOut, true);
  assert.equal(result.exitCode, null);
  assert.equal(result.signal, 'SIGKILL');
  assert.equal(result.timeoutMs, 50);
  assert.ok(Date.now() - startedAt < 500);
});

test('spawnProcessRunner closes provider stdin for noninteractive invocations', async () => {
  const { spawnProcessRunner } = require('../../lib/agent-cli-provider');

  const result = await spawnProcessRunner()(
    {
      binary: process.execPath,
      args: [
        '-e',
        [
          'process.stdin.setEncoding("utf8");',
          'process.stdin.on("data", () => {});',
          'process.stdin.on("end", () => { process.stdout.write("stdin closed"); });',
          'process.stdin.resume();',
        ].join(' '),
      ],
      env: {},
      cleanupMetadata: [],
      warnings: [],
      redactions: [],
    },
    { timeoutMs: 300 }
  );

  assert.equal(result.exitCode, 0);
  assert.equal(result.timedOut, false);
  assert.equal(result.stdout, 'stdin closed');
});
