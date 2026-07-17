'use strict';

const assert = require('assert');
const { ARTIFACT, fakeEngine, registry, request, workerWith } = require('./helpers');

function deferredRegistry() {
  const backingRegistry = registry();
  const profile = backingRegistry.resolve('isolation.worktree@1', 'provider.default@1');
  const calls = [];
  let resolveProfile;
  const resolution = new Promise((resolve) => {
    resolveProfile = resolve;
  });
  return {
    calls,
    profileRegistry: Object.freeze({
      bounds: backingRegistry.bounds,
      resolve(isolationProfile, providerProfile) {
        calls.push([isolationProfile, providerProfile]);
        return resolution;
      },
    }),
    resolveProfile: () => resolveProfile(profile),
  };
}

function deferredStart(source = 'prompt', overrides = {}) {
  const engine = fakeEngine();
  const deferred = deferredRegistry();
  const worker = workerWith(engine, {
    profileRegistry: deferred.profileRegistry,
    ...overrides,
  });
  const input = request(source);
  return { deferred, engine, input, starting: worker.start(input), worker };
}

async function finishDeferredStart(context) {
  context.deferred.resolveProfile();
  await context.starting;
  return context.engine.calls.starts[0].request;
}

describe('legacy cluster worker request snapshot', () => {
  it('snapshots prompt input before asynchronous profile preparation', async () => {
    const context = deferredStart();
    context.input.prompt = 'MUTATED';
    const received = await finishDeferredStart(context);

    assert.strictEqual(received.prompt, 'Run the bounded task');
    assert.notStrictEqual(received, context.input);
    assert.ok(Object.isFrozen(received));
    await context.worker.stop();
  });

  it('excludes forbidden fields added after synchronous validation', async () => {
    const context = deferredStart();
    context.input.command = 'rm -rf /';
    context.input.credentials = { token: 'raw-secret' };
    const received = await finishDeferredStart(context);

    assert.strictEqual(received.command, undefined);
    assert.strictEqual(received.credentials, undefined);
    assert.deepStrictEqual(Object.keys(received).sort(), Object.keys(request()).sort());
    await context.worker.stop();
  });

  it('snapshots registry handles before asynchronous profile resolution', async () => {
    const context = deferredStart();
    context.input.isolationProfile = 'isolation.none@1';
    context.input.providerProfile = 'provider.mutated@1';
    const received = await finishDeferredStart(context);

    assert.deepStrictEqual(context.deferred.calls, [
      ['isolation.worktree@1', 'provider.default@1'],
    ]);
    assert.strictEqual(received.isolationProfile, 'isolation.worktree@1');
    assert.strictEqual(received.providerProfile, 'provider.default@1');
    await context.worker.stop();
  });

  it('snapshots and freezes artifact receipts before asynchronous preparation', async () => {
    let staged;
    const context = deferredStart('artifact', {
      artifactResolver: {
        stage(artifacts) {
          staged = artifacts;
          return { artifacts: [] };
        },
      },
    });
    context.input.artifacts[0] = { ...ARTIFACT, byteLength: 999 };
    context.input.artifacts.push({ bytes: 'raw-inline-bytes' });
    const received = await finishDeferredStart(context);

    assert.deepStrictEqual(staged, [ARTIFACT]);
    assert.ok(Object.isFrozen(staged));
    assert.ok(Object.isFrozen(staged[0]));
    assert.deepStrictEqual(received.artifacts, [ARTIFACT]);
    await context.worker.stop();
  });
});

describe('legacy cluster worker cancellation', () => {
  it('claims start synchronously and prevents concurrent duplicate allocation', async () => {
    const engine = fakeEngine();
    let resolveProfile;
    const pendingProfile = new Promise((resolve) => {
      resolveProfile = resolve;
    });
    const canonical = registry().resolve('isolation.worktree@1', 'provider.default@1');
    const worker = workerWith(engine, { profileRegistry: { resolve: () => pendingProfile } });
    const first = worker.start(request());
    await assert.rejects(worker.start(request()), /exactly one start/);
    resolveProfile(canonical);
    await first;
    assert.strictEqual(engine.calls.starts.length, 1);
    await worker.stop();
  });

  it('cancels unresolved profile preparation without allocating an engine', async () => {
    const engine = fakeEngine();
    let resolveProfile;
    const pendingProfile = new Promise((resolve) => {
      resolveProfile = resolve;
    });
    const canonical = registry().resolve('isolation.worktree@1', 'provider.default@1');
    const worker = workerWith(engine, { profileRegistry: { resolve: () => pendingProfile } });
    const starting = worker.start(request());
    const receipt = await Promise.race([
      worker.stop(),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('stop waited for profile resolution')), 25)
      ),
    ]);
    assert.strictEqual(receipt.state, 'stopped');
    resolveProfile(canonical);
    assert.strictEqual((await starting).state, 'stopped');
    assert.strictEqual(engine.calls.starts.length, 0);
  });

  it('applies the registry execution budget while profile resolution is unresolved', async () => {
    const engine = fakeEngine();
    const boundedRegistry = registry({ executionMs: 5 });
    const worker = workerWith(engine, {
      profileRegistry: Object.freeze({
        bounds: boundedRegistry.bounds,
        resolve: () => new Promise(() => {}),
      }),
    });
    const starting = worker.start(request());
    const receipt = await worker.result();
    assert.strictEqual(receipt.state, 'timed_out');
    assert.strictEqual((await starting).state, 'timed_out');
    assert.strictEqual(engine.calls.starts.length, 0);
  });
});

describe('legacy cluster worker artifact cancellation', () => {
  it('cancels unresolved artifact preparation without allocating an engine', async () => {
    const engine = fakeEngine();
    let resolveArtifacts;
    const worker = workerWith(engine, {
      artifactResolver: {
        stage() {
          return new Promise((resolve) => {
            resolveArtifacts = resolve;
          });
        },
      },
    });
    const starting = worker.start(request('artifact'));
    await new Promise((resolve) => setImmediate(resolve));
    const receipt = await Promise.race([
      worker.stop(),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('stop waited for artifact staging')), 25)
      ),
    ]);
    assert.strictEqual(receipt.state, 'stopped');
    resolveArtifacts({ artifacts: [] });
    assert.strictEqual((await starting).state, 'stopped');
    assert.strictEqual(engine.calls.starts.length, 0);
  });

  it('lets explicit stop win while artifact receipt collection is unresolved', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine, {
      artifactReceiptSink: { collect: () => new Promise(() => {}) },
    });
    await worker.start(request());
    engine.emit({ type: 'complete', summary: 'pending receipt collection', artifacts: [] });
    const receipt = await Promise.race([
      worker.stop(),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('stop lost to receipt collection')), 25)
      ),
    ]);
    assert.strictEqual(receipt.state, 'stopped');
    assert.strictEqual((await worker.result()).state, 'stopped');
  });

  it('keeps the execution deadline armed while artifact receipt collection is unresolved', async () => {
    const engine = fakeEngine();
    const worker = workerWith(engine, {
      profileRegistry: registry({ executionMs: 5 }),
      artifactReceiptSink: { collect: () => new Promise(() => {}) },
    });
    await worker.start(request());
    engine.emit({ type: 'complete', summary: 'pending receipt collection', artifacts: [] });
    const receipt = await worker.result();
    assert.strictEqual(receipt.state, 'timed_out');
    assert.strictEqual(receipt.outcome.code, 'timeout');
  });
});
