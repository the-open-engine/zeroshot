'use strict';

const assert = require('assert');
const { CANCELLED, runCancellable } = require('../../lib/cluster-worker/runtime-support');
const { ARTIFACT, fakeEngine, registry, request, workerWith } = require('./helpers');

describe('legacy cluster worker cancellation cleanup', () => {
  it('aborts profile resolution and releases a descriptor that arrives late', async () => {
    const engine = fakeEngine();
    const canonical = registry().resolve('isolation.worktree@1', 'provider.default@1');
    let resolveProfile;
    let observedSignal;
    let releasedProfile;
    const worker = workerWith(engine, {
      profileRegistry: {
        resolve(_isolationProfile, _providerProfile, context) {
          observedSignal = context.signal;
          return new Promise((resolve) => {
            resolveProfile = resolve;
          });
        },
        release(profile, context) {
          releasedProfile = { profile, signal: context.signal };
        },
      },
    });

    const starting = worker.start(request());
    await new Promise((resolve) => setImmediate(resolve));
    await worker.stop();
    assert.strictEqual(observedSignal.aborted, true);

    resolveProfile(canonical);
    await new Promise((resolve) => setImmediate(resolve));
    assert.strictEqual(releasedProfile.profile, canonical);
    assert.strictEqual(releasedProfile.signal, observedSignal);
    assert.strictEqual((await starting).state, 'stopped');
  });

  it('aborts artifact staging and cleans a manifest that arrives late', async () => {
    const engine = fakeEngine();
    let resolveArtifacts;
    let observedSignal;
    let cleanedManifest;
    const worker = workerWith(engine, {
      artifactResolver: {
        stage(_artifacts, context) {
          observedSignal = context.signal;
          return new Promise((resolve) => {
            resolveArtifacts = resolve;
          });
        },
        cleanup(manifest, context) {
          cleanedManifest = { manifest, signal: context.signal };
        },
      },
    });

    const starting = worker.start(request('artifact'));
    await new Promise((resolve) => setImmediate(resolve));
    await worker.stop();
    assert.strictEqual(observedSignal.aborted, true);

    const lateManifest = { artifacts: [], root: 'late-staging-root' };
    resolveArtifacts(lateManifest);
    await new Promise((resolve) => setImmediate(resolve));
    assert.strictEqual(cleanedManifest.manifest, lateManifest);
    assert.strictEqual(cleanedManifest.signal, observedSignal);
    assert.strictEqual((await starting).state, 'stopped');
    assert.strictEqual(engine.calls.starts.length, 0);
  });

  it('aborts receipt collection and cleans receipts that arrive late', async () => {
    const engine = fakeEngine();
    let resolveReceipts;
    let observedSignal;
    let cleanedReceipts;
    const worker = workerWith(engine, {
      artifactReceiptSink: {
        collect(_declared, context) {
          observedSignal = context.signal;
          return new Promise((resolve) => {
            resolveReceipts = resolve;
          });
        },
        cleanup(receipts, context) {
          cleanedReceipts = { receipts, signal: context.signal };
        },
      },
    });

    await worker.start(request());
    engine.emit({ type: 'complete', summary: 'pending receipts', artifacts: [ARTIFACT] });
    await new Promise((resolve) => setImmediate(resolve));
    await worker.stop();
    assert.strictEqual(observedSignal.aborted, true);

    resolveReceipts([ARTIFACT]);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepStrictEqual(cleanedReceipts.receipts, [ARTIFACT]);
    assert.strictEqual(cleanedReceipts.signal, observedSignal);
    assert.strictEqual((await worker.result()).state, 'stopped');
  });

  it('reports a late artifact cleanup failure after cancellation', async () => {
    const engine = fakeEngine();
    const failures = [];
    let resolveArtifacts;
    const worker = workerWith(engine, {
      cleanupFailureReporter: (failure) => failures.push(failure),
      artifactResolver: {
        stage() {
          return new Promise((resolve) => {
            resolveArtifacts = resolve;
          });
        },
        cleanup() {
          throw new Error('staging root removal failed');
        },
      },
    });

    const starting = worker.start(request('artifact'));
    await new Promise((resolve) => setImmediate(resolve));
    await worker.stop();
    resolveArtifacts({ artifacts: [], root: 'late-staging-root' });
    await new Promise((resolve) => setImmediate(resolve));

    assert.strictEqual((await starting).state, 'stopped');
    assert.strictEqual(failures.length, 1);
    assert.strictEqual(failures[0].phase, 'artifact_staging');
    assert.strictEqual(failures[0].kind, 'cleanup');
    assert.strictEqual(failures[0].clusterId, 'cluster-1');
    assert.match(failures[0].error.message, /staging root removal failed/);
  });

  it('reports a late profile resolution rejection after cancellation', async () => {
    const engine = fakeEngine();
    const failures = [];
    let rejectProfile;
    const worker = workerWith(engine, {
      cleanupFailureReporter: (failure) => failures.push(failure),
      profileRegistry: {
        resolve() {
          return new Promise((_, reject) => {
            rejectProfile = reject;
          });
        },
      },
    });

    const starting = worker.start(request());
    await new Promise((resolve) => setImmediate(resolve));
    await worker.stop();
    rejectProfile(new Error('late registry transport failure'));
    await new Promise((resolve) => setImmediate(resolve));

    assert.strictEqual((await starting).state, 'stopped');
    assert.strictEqual(failures.length, 1);
    assert.strictEqual(failures[0].phase, 'profile_resolution');
    assert.strictEqual(failures[0].kind, 'operation');
    assert.strictEqual(failures[0].clusterId, 'cluster-1');
    assert.match(failures[0].error.message, /late registry transport failure/);
  });

  it('emits a process warning when the injected failure reporter itself throws', async () => {
    const controller = new AbortController();
    let resolveOperation;
    const operation = new Promise((resolve) => {
      resolveOperation = resolve;
    });
    const warningPromise = new Promise((resolve) => process.once('warning', resolve));
    const cancelled = runCancellable(operation, {
      signal: controller.signal,
      cleanup() {
        throw new Error('late cleanup failed');
      },
      reportFailure() {
        throw new Error('reporter unavailable');
      },
      failureContext: { phase: 'artifact_staging', clusterId: 'cluster-1' },
    });

    controller.abort();
    assert.strictEqual(await cancelled, CANCELLED);
    resolveOperation({ root: 'late-staging-root' });

    const warning = await warningPromise;
    assert.strictEqual(warning.code, 'ZEROSHOT_LEGACY_CLEANUP_FAILURE');
    assert.match(warning.message, /artifact_staging cleanup failed/);
    assert.match(warning.message, /late cleanup failed/);
    assert.match(warning.message, /reporter unavailable/);
  });
});
