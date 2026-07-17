'use strict';

const assert = require('assert');
const { ARTIFACT, fakeEngine, registry, request, workerWith } = require('./helpers');

describe('legacy cluster worker facade', () => {
  it('exposes only the bounded five-operation lifecycle', () => {
    const worker = workerWith(fakeEngine());
    assert.deepStrictEqual(Object.keys(worker), ['start', 'status', 'events', 'stop', 'result']);
    assert.strictEqual(worker.guidance, undefined);
    assert.strictEqual(worker.attach, undefined);
    assert.strictEqual(worker.permissions, undefined);
  });

  it('normalizes successful engine truth without exposing raw output', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine);
    await worker.start(request());
    engine.emit({
      type: 'complete',
      summary: 'bounded success',
      rawOutput: 'must not escape',
      artifacts: [],
    });
    const receipt = await worker.result();
    assert.deepStrictEqual(receipt, {
      state: 'completed',
      clusterId: 'cluster-1',
      finishedAt: 100,
      result: { summary: 'bounded success', status: 'succeeded', artifacts: [] },
    });
    assert.strictEqual(JSON.stringify(receipt).includes('must not escape'), false);
  });

  it('does not accept engine-declared artifact receipts without a receipt sink', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine);
    await worker.start(request());
    engine.emit({
      type: 'complete',
      result: { summary: 'done', status: 'succeeded', artifacts: [ARTIFACT] },
    });
    assert.deepStrictEqual((await worker.result()).result.artifacts, []);
  });

  it('normalizes declared failure and malformed output through the closed algebra', async () => {
    const failedEngine = fakeEngine();
    const failedWorker = workerWith(failedEngine);
    await failedWorker.start(request());
    failedEngine.emit({ type: 'failed', code: 'refusal', reason: 'policy_denied' });
    assert.deepStrictEqual((await failedWorker.result()).outcome, {
      status: 'error',
      code: 'refusal',
      reason: 'policy_denied',
    });

    const malformedEngine = fakeEngine();
    const malformedWorker = workerWith(malformedEngine);
    await malformedWorker.start(request());
    malformedEngine.emit({
      type: 'complete',
      result: { summary: 'inline bytes', status: 'succeeded', artifacts: [{ bytes: 'bad' }] },
    });
    const malformed = await malformedWorker.result();
    assert.strictEqual(malformed.state, 'malformed');
    assert.deepStrictEqual(malformed.outcome, {
      status: 'error',
      code: 'malformed',
      reason: 'malformed_result',
    });

    for (const terminalEvent of [
      { type: 'complete' },
      { type: 'failed', code: 'unknown', reason: 'declared_failure' },
      { type: 'failed', code: 'crash', reason: 'policy_denied' },
    ]) {
      const invalidEngine = fakeEngine();
      const invalidWorker = workerWith(invalidEngine);
      await invalidWorker.start(request());
      invalidEngine.emit(terminalEvent);
      const invalid = await invalidWorker.result();
      assert.strictEqual(invalid.state, 'malformed');
      assert.deepStrictEqual(invalid.outcome, {
        status: 'error',
        code: 'malformed',
        reason: 'malformed_result',
      });
    }
  });

  it('fails closed when synchronous engine-truth observation throws', async () => {
    const engine = fakeEngine();
    const failingAdapter = Object.freeze({
      ...engine.adapter,
      status() {
        throw new Error('durable ledger unavailable');
      },
    });
    const worker = workerWith({ ...engine, adapter: failingAdapter });
    await worker.start(request());
    assert.strictEqual(worker.status().state, 'failed');
    assert.deepStrictEqual((await worker.result()).outcome, {
      status: 'error',
      code: 'crash',
      reason: 'declared_failure',
    });
  });

  it('rejects asynchronous engine status ports without losing their rejection', async () => {
    const engine = fakeEngine();
    const invalidAdapter = Object.freeze({
      ...engine.adapter,
      status() {
        return Promise.reject(new Error('late ledger rejection'));
      },
    });
    const worker = workerWith({ ...engine, adapter: invalidAdapter });
    await worker.start(request());
    assert.strictEqual(worker.status().state, 'failed');
    await new Promise((resolve) => setImmediate(resolve));
    assert.strictEqual((await worker.result()).state, 'failed');
  });

  it('rejects missing isolation before engine or artifact allocation', async () => {
    const engine = fakeEngine();
    let artifactAllocations = 0;
    const worker = workerWith(engine, {
      profileRegistry: {
        resolve() {
          return Object.freeze({
            plan: Object.freeze({ isolation: 'none', delivery: 'none', autoMerge: false }),
            bounds: Object.freeze({ executionMs: 10 }),
          });
        },
      },
      artifactResolver: {
        stage() {
          artifactAllocations += 1;
          return { artifacts: [] };
        },
      },
    });
    await assert.rejects(worker.start(request()), /non-isolated execution/);
    assert.strictEqual(worker.status().clusterId, null);
    assert.strictEqual(engine.calls.starts.length, 0);
    assert.strictEqual(artifactAllocations, 0);
  });

  it('rejects mutable isolated profile descriptors before engine allocation', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine, {
      profileRegistry: {
        resolve() {
          return {
            plan: Object.freeze({ isolation: 'worktree', delivery: 'none', autoMerge: false }),
            deployment: Object.freeze({ worktree: true }),
            provider: Object.freeze({}),
            bounds: Object.freeze({ executionMs: 10, shutdownMs: 10, frameBytes: 1024 }),
          };
        },
      },
    });
    await assert.rejects(worker.start(request()), /frozen deployment descriptor/);
    assert.strictEqual(engine.calls.starts.length, 0);
  });

  it('stages artifact receipts and emits only receipt-sink outputs', async () => {
    const engine = fakeEngine();
    let staged;
    const worker = workerWith(engine, {
      artifactResolver: {
        stage(artifacts) {
          staged = artifacts;
          return { artifacts, internal: 'read-only-staging-handle' };
        },
      },
      artifactReceiptSink: {
        collect() {
          return [ARTIFACT];
        },
      },
    });
    await worker.start(request('artifact'));
    assert.deepStrictEqual(staged, [ARTIFACT]);
    assert.ok(Object.isFrozen(engine.calls.starts[0].artifactManifest));
    engine.emit({
      type: 'complete',
      summary: 'artifact output complete',
      artifacts: [{ bytes: 'engine-private' }],
    });
    assert.deepStrictEqual((await worker.result()).result.artifacts, [ARTIFACT]);
  });

  it('makes timeout the single terminal authority and requests engine stop', async () => {
    const engine = fakeEngine();
    let timerCallback;
    const timers = {
      setTimeout(callback) {
        timerCallback = callback;
        return 1;
      },
      clearTimeout() {},
    };
    const worker = workerWith(engine, { timers });
    await worker.start(request());
    timerCallback();
    engine.emit({ type: 'complete', summary: 'late completion' });
    const receipt = await worker.result();
    assert.strictEqual(receipt.state, 'timed_out');
    assert.strictEqual(receipt.outcome.code, 'timeout');
    await new Promise((resolve) => setImmediate(resolve));
    assert.strictEqual(engine.calls.stops, 1);
  });

  it('makes explicit cancellation final without claiming rollback', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine);
    await worker.start(request());
    const receipt = await worker.stop();
    engine.emit({ type: 'complete', summary: 'late completion' });
    assert.deepStrictEqual(receipt.stop, {
      requested: true,
      effective: true,
      externalEffectsRolledBack: false,
    });
    assert.strictEqual((await worker.result()).state, 'stopped');
  });

  it('records an ineffective stop without claiming rollback when engine stop fails', async () => {
    const engine = fakeEngine();
    const failingAdapter = Object.freeze({
      ...engine.adapter,
      stop() {
        throw new Error('engine stop unavailable');
      },
    });
    const worker = workerWith({ ...engine, adapter: failingAdapter });
    await worker.start(request());
    const receipt = await worker.stop();
    assert.deepStrictEqual(receipt.stop, {
      requested: true,
      effective: false,
      externalEffectsRolledBack: false,
    });
  });

  it('bounds explicit engine stop with the registry shutdown deadline', async () => {
    const engine = fakeEngine();
    const hangingAdapter = Object.freeze({
      ...engine.adapter,
      stop() {
        return new Promise(() => {});
      },
    });
    const worker = workerWith(
      { ...engine, adapter: hangingAdapter },
      { profileRegistry: registry({ shutdownMs: 5 }) }
    );
    await worker.start(request());
    const receipt = await worker.stop();
    assert.strictEqual(receipt.state, 'stopped');
    assert.strictEqual(receipt.stop.effective, false);
  });
});
