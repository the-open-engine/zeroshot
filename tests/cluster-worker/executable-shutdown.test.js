'use strict';

const assert = require('assert');
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { PassThrough } = require('stream');
const { runClusterWorkerExecutable } = require('../../lib/cluster-worker/executable');
const { createLegacyClusterWorker } = require('../../lib/cluster-worker');
const { registry, request } = require('./helpers');

function capture() {
  const frames = [];
  return {
    frames,
    stream: { write: (value) => frames.push(JSON.parse(value)) },
  };
}

describe('legacy cluster worker executable timeout cleanup', () => {
  it('waits for in-flight timeout cleanup before close settles', async () => {
    const input = new PassThrough();
    let resolveStop;
    let stopSettled = false;
    const worker = createLegacyClusterWorker({
      profileRegistry: registry({ executionMs: 5, shutdownMs: 100 }),
      engineAdapter: Object.freeze({
        start: () => new Promise(() => {}),
        status: () => ({ state: 'starting' }),
        stop: () =>
          new Promise((resolve) => {
            resolveStop = () => {
              stopSettled = true;
              resolve({ effective: true });
            };
          }),
      }),
      idFactory: () => 'cluster-timeout-cleanup',
    });
    const output = capture();
    const runtime = runClusterWorkerExecutable({
      input,
      output: output.stream,
      diagnostics: { write() {} },
      worker,
      shutdownMs: 100,
    });

    await runtime.dispatch({ id: 'start', method: 'start', params: { request: request() } });
    assert.strictEqual(
      output.frames.find((frame) => frame.id === 'start').result.state,
      'timed_out'
    );

    let closeSettled = false;
    const closing = runtime.close().then(() => {
      closeSettled = true;
    });
    await new Promise((resolve) => setImmediate(resolve));
    assert.strictEqual(closeSettled, false);
    assert.strictEqual(stopSettled, false);

    resolveStop();
    await closing;
    assert.strictEqual(stopSettled, true);
    input.destroy();
  });
});

describe('legacy cluster worker executable EOF cancellation', () => {
  it('requests bounded explicit cancellation on EOF', async () => {
    const input = new PassThrough();
    let stopped = 0;
    const worker = {
      start: () => ({ state: 'running', terminal: false }),
      events() {},
      stop() {
        stopped += 1;
      },
      result() {},
    };
    runClusterWorkerExecutable({
      input,
      output: capture().stream,
      diagnostics: { write() {} },
      worker,
      shutdownMs: 50,
    });
    input.end(`${JSON.stringify({ id: 1, method: 'start', params: { request: request() } })}\n`);
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.strictEqual(stopped, 1);
  });

  it('requests cancellation on EOF while start never settles', async () => {
    const input = new PassThrough();
    let stopped = 0;
    const worker = {
      start: () => new Promise(() => {}),
      events() {},
      stop() {
        stopped += 1;
        return new Promise(() => {});
      },
      result() {},
    };
    runClusterWorkerExecutable({
      input,
      output: capture().stream,
      diagnostics: { write() {} },
      worker,
      shutdownMs: 10,
    });
    input.end(`${JSON.stringify({ id: 1, method: 'start', params: { request: request() } })}\n`);
    await new Promise((resolve) => setTimeout(resolve, 25));
    assert.strictEqual(stopped, 1);
  });

  it('keeps the process alive until a late-allocated cluster is cleaned up', async () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'cluster-worker-cleanup-'));
    const markerPath = path.join(directory, 'stopped');
    const fixture = path.join(__dirname, 'fixtures', 'late-allocation-worker.js');
    const child = spawn(process.execPath, [fixture, markerPath], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stderr = '';
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    const exited = new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (code, signal) => resolve({ code, signal }));
    });
    const timeout = setTimeout(() => child.kill('SIGKILL'), 2000);

    try {
      child.stdin.end(
        `${JSON.stringify({ id: 1, method: 'start', params: { request: request() } })}\n`
      );
      const exit = await exited;
      assert.deepStrictEqual(exit, { code: 0, signal: null }, stderr);
      assert.strictEqual(fs.readFileSync(markerPath, 'utf8'), 'stopped\n');
    } finally {
      clearTimeout(timeout);
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });

  for (const signal of ['SIGINT', 'SIGTERM']) {
    it(`exits after ${signal} cleanup while the parent keeps stdin open`, async () => {
      const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'cluster-worker-signal-'));
      const markerPath = path.join(directory, 'stopped');
      const fixture = path.join(__dirname, 'fixtures', 'late-allocation-worker.js');
      const child = spawn(process.execPath, [fixture, markerPath], {
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      let stderr = '';
      let resolveReady;
      const ready = new Promise((resolve) => {
        resolveReady = resolve;
      });
      child.stderr.on('data', (chunk) => {
        stderr += chunk.toString();
        if (stderr.includes('ready\n')) resolveReady();
      });
      const exited = new Promise((resolve, reject) => {
        child.once('error', reject);
        child.once('close', (code, exitSignal) => resolve({ code, signal: exitSignal }));
      });
      const timeout = setTimeout(() => child.kill('SIGKILL'), 2000);

      try {
        child.stdin.write(
          `${JSON.stringify({ id: 1, method: 'start', params: { request: request() } })}\n`
        );
        await Promise.race([
          ready,
          exited.then((exit) => {
            throw new Error(
              `Child exited before signal handler readiness: ${JSON.stringify(exit)}`
            );
          }),
        ]);
        await new Promise((resolve) => setTimeout(resolve, 200));
        assert.strictEqual(child.kill(signal), true);
        const exit = await exited;
        assert.deepStrictEqual(exit, { code: 0, signal: null }, stderr);
        assert.strictEqual(fs.readFileSync(markerPath, 'utf8'), 'stopped\n');
      } finally {
        clearTimeout(timeout);
        if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
        fs.rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
