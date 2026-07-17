'use strict';

const assert = require('assert');
const { PassThrough } = require('stream');
const {
  createCommandDispatcher,
  runClusterWorkerExecutable,
} = require('../../lib/cluster-worker/executable');
const { createLegacyClusterWorker } = require('../../lib/cluster-worker');
const { fakeEngine, registry, request } = require('./helpers');

function capture() {
  const frames = [];
  return {
    frames,
    stream: { write: (value) => frames.push(JSON.parse(value)) },
  };
}

describe('legacy cluster worker executable', () => {
  it('correlates the five closed commands and emits lifecycle events', async () => {
    const engine = fakeEngine();
    const worker = createLegacyClusterWorker({
      profileRegistry: registry(),
      engineAdapter: engine.adapter,
      idFactory: () => 'cluster-1',
      clock: () => 10,
    });
    const output = capture();
    const dispatcher = createCommandDispatcher({
      worker,
      output: output.stream,
      diagnostics: { write() {} },
    });
    await dispatcher.dispatch({ id: 'events', method: 'events', params: {} });
    await dispatcher.dispatch({ id: 'start', method: 'start', params: { request: request() } });
    await dispatcher.dispatch({ id: 'status', method: 'status', params: {} });
    engine.emit({ type: 'complete', summary: 'done' });
    await dispatcher.dispatch({ id: 'result', method: 'result', params: {} });
    await dispatcher.dispatch({ id: 'stop', method: 'stop', params: {} });
    await new Promise((resolve) => setImmediate(resolve));
    const responses = output.frames.filter((frame) => frame.type === 'response');
    assert.deepStrictEqual(
      responses.map((frame) => frame.id),
      ['events', 'start', 'status', 'result', 'stop']
    );
    assert.ok(output.frames.some((frame) => frame.type === 'event'));
    assert.ok(output.frames.every((frame) => JSON.parse(JSON.stringify(frame))));
  });

  it('rejects unknown methods, writable params, arrays, and duplicate start', async () => {
    const worker = {
      start() {
        return { state: 'running' };
      },
      status() {
        return { terminal: false };
      },
      events() {},
      stop() {},
      result() {},
    };
    const output = capture();
    const dispatcher = createCommandDispatcher({
      worker,
      output: output.stream,
      diagnostics: { write() {} },
    });
    await dispatcher.dispatch([]);
    await dispatcher.dispatch({ id: 1, method: 'attach', params: {} });
    await dispatcher.dispatch({ id: 2, method: 'status', params: { writable: true } });
    await dispatcher.dispatch({ id: 3, method: 'start', params: { request: request() } });
    await dispatcher.dispatch({ id: 4, method: 'start', params: { request: request() } });
    assert.strictEqual(output.frames.filter((frame) => frame.ok === false).length, 4);
    assert.strictEqual(output.frames.at(-1).error.code, 'DUPLICATE_START');
  });

  it('waits for terminal truth without waiting for an in-flight start to settle', async () => {
    let finishStart;
    let finishResult;
    let resultCalls = 0;
    const worker = {
      start() {
        return new Promise((resolve) => {
          finishStart = resolve;
        });
      },
      status() {
        return { terminal: false };
      },
      events() {
        return { return() {} };
      },
      stop() {},
      result() {
        resultCalls += 1;
        return new Promise((resolve) => {
          finishResult = resolve;
        });
      },
    };
    const output = capture();
    const dispatcher = createCommandDispatcher({
      worker,
      output: output.stream,
      diagnostics: { write() {} },
    });
    const starting = dispatcher.dispatch({
      id: 'start',
      method: 'start',
      params: { request: request() },
    });
    const result = dispatcher.dispatch({ id: 'result', method: 'result', params: {} });
    await new Promise((resolve) => setImmediate(resolve));
    assert.strictEqual(resultCalls, 1);
    finishStart({ state: 'running' });
    finishResult({ state: 'completed' });
    await Promise.all([starting, result]);
    assert.strictEqual(resultCalls, 1);
  });

  it('returns terminal truth when engine start hangs past the execution deadline', async () => {
    const worker = createLegacyClusterWorker({
      profileRegistry: registry({ executionMs: 5, shutdownMs: 5 }),
      engineAdapter: Object.freeze({
        start() {
          return new Promise(() => {});
        },
        status() {
          return { state: 'starting' };
        },
        stop() {
          return { effective: true };
        },
      }),
      idFactory: () => 'cluster-timeout',
    });
    const output = capture();
    const dispatcher = createCommandDispatcher({
      worker,
      output: output.stream,
      diagnostics: { write() {} },
    });
    dispatcher.dispatch({ id: 'start', method: 'start', params: { request: request() } });
    dispatcher.dispatch({ id: 'result', method: 'result', params: {} });
    await new Promise((resolve) => setTimeout(resolve, 25));
    const result = output.frames.find((frame) => frame.id === 'result');
    assert.strictEqual(result.ok, true);
    assert.strictEqual(result.result.state, 'timed_out');
  });

  it('stops a resource while engine start remains in flight', async () => {
    let stopCalls = 0;
    const worker = createLegacyClusterWorker({
      profileRegistry: registry({ executionMs: 1000, shutdownMs: 5 }),
      engineAdapter: Object.freeze({
        start() {
          return new Promise(() => {});
        },
        status() {
          return { state: 'starting' };
        },
        stop() {
          stopCalls += 1;
          return { effective: true };
        },
      }),
      idFactory: () => 'cluster-stopped-during-start',
    });
    const output = capture();
    const dispatcher = createCommandDispatcher({
      worker,
      output: output.stream,
      diagnostics: { write() {} },
    });
    dispatcher.dispatch({ id: 'start', method: 'start', params: { request: request() } });
    await new Promise((resolve) => setImmediate(resolve));
    dispatcher.dispatch({ id: 'stop', method: 'stop', params: {} });
    await new Promise((resolve) => setTimeout(resolve, 25));
    const stop = output.frames.find((frame) => frame.id === 'stop');
    assert.strictEqual(stop.ok, true);
    assert.strictEqual(stop.result.state, 'stopped');
    assert.strictEqual(stopCalls, 1);
  });

  it('returns canonical failed truth after engine start rejects', async () => {
    const worker = createLegacyClusterWorker({
      profileRegistry: registry(),
      engineAdapter: Object.freeze({
        start() {
          throw new Error('engine allocation failed');
        },
        status() {
          return { state: 'failed' };
        },
        stop() {
          return { effective: false };
        },
      }),
      idFactory: () => 'cluster-failed-start',
    });
    const output = capture();
    const dispatcher = createCommandDispatcher({
      worker,
      output: output.stream,
      diagnostics: { write() {} },
    });
    await dispatcher.dispatch({ id: 'start', method: 'start', params: { request: request() } });
    await dispatcher.dispatch({ id: 'result', method: 'result', params: {} });
    const start = output.frames.find((frame) => frame.id === 'start');
    const result = output.frames.find((frame) => frame.id === 'result');
    assert.strictEqual(start.ok, false);
    assert.strictEqual(result.ok, true);
    assert.strictEqual(result.result.state, 'failed');
    assert.strictEqual(result.result.outcome.code, 'crash');
  });

  it('bounds input before buffering and recovers at the next NDJSON frame', async () => {
    const input = new PassThrough();
    const output = capture();
    const worker = {
      status() {
        return { state: 'idle', terminal: false };
      },
      events() {},
      start() {},
      stop() {},
      result() {},
    };
    runClusterWorkerExecutable({
      input,
      output: output.stream,
      diagnostics: { write() {} },
      worker,
      frameBytes: 64,
    });
    input.write(`${'x'.repeat(100)}\n`);
    input.write('{not-json}\n');
    input.write(`${JSON.stringify({ id: 1, method: 'status', params: {} })}\n`);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepStrictEqual(
      output.frames.map((frame) => (frame.ok ? 'OK' : frame.error.code)),
      ['FRAME_TOO_LARGE', 'MALFORMED_JSON', 'OK']
    );
    input.end();
  });
});
