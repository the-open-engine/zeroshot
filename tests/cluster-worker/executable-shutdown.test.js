'use strict';

const assert = require('assert');
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
});
