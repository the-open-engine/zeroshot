'use strict';

const assert = require('assert');
const { ARTIFACT, fakeEngine, registry, request, workerWith } = require('./helpers');

describe('legacy cluster worker facade admission', () => {
  it('permits a clean retry after synchronous profile rejection', async () => {
    const engine = fakeEngine();
    const backingRegistry = registry();
    let rejectFirst = true;
    const worker = workerWith(engine, {
      profileRegistry: {
        resolve(isolationProfile, providerProfile) {
          if (rejectFirst) {
            rejectFirst = false;
            throw new Error('Unknown isolation profile: isolation.unknown@1');
          }
          return backingRegistry.resolve(isolationProfile, providerProfile);
        },
      },
    });

    await assert.rejects(worker.start(request()), /Unknown isolation profile/);
    assert.deepStrictEqual(worker.status(), {
      state: 'idle',
      clusterId: null,
      sequence: 0,
      stopRequested: false,
      terminal: false,
    });
    assert.throws(() => worker.result(), /Worker has not started/);
    await assert.rejects(worker.stop(), /Worker has not started/);

    await worker.start(request());
    assert.strictEqual(engine.calls.starts.length, 1);
    await worker.stop();
  });

  it('rejects artifact input without a real resolver before allocation', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine);

    await assert.rejects(worker.start(request('artifact')), /ArtifactResolver is required/);

    assert.strictEqual(engine.calls.starts.length, 0);
    assert.strictEqual(worker.status().clusterId, null);
    assert.throws(() => worker.result(), /Worker has not started/);
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

  it('fails closed when an engine does not stage artifact input', async () => {
    const engine = fakeEngine();
    const unstagedAdapter = Object.freeze({
      ...engine.adapter,
      start(value) {
        engine.calls.starts.push(value);
        return { clusterId: value.clusterId };
      },
    });
    const worker = workerWith(
      { ...engine, adapter: unstagedAdapter },
      {
        artifactResolver: {
          stage() {
            return { artifacts: [] };
          },
        },
      }
    );

    await assert.rejects(worker.start(request('artifact')), /without staging artifact input/);
    assert.strictEqual(engine.calls.stops, 1);
    assert.strictEqual((await worker.result()).state, 'failed');
  });
});
