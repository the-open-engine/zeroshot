'use strict';

const { DEFAULT_BOUNDS } = require('./profiles');
const { reportLateFailure, stopEngineWithinBound } = require('./runtime-support');

function stopEngine(runtime) {
  runtime.engineStopPromise ||= stopEngineWithinBound({
    engineAdapter: runtime.engineAdapter,
    timers: runtime.timers,
    shutdownMs: runtime.profile?.bounds.shutdownMs || DEFAULT_BOUNDS.shutdownMs,
  });
  return runtime.engineStopPromise;
}

async function releaseEngine(runtime) {
  if (typeof runtime.engineAdapter.close !== 'function') return;
  if (!runtime.engineReleasePromise) {
    try {
      runtime.engineReleasePromise = Promise.resolve(runtime.engineAdapter.close()).then(
        undefined,
        (error) => {
          reportLateFailure(runtime.cleanupFailureReporter, {
            phase: 'engine_release',
            kind: 'cleanup',
            clusterId: runtime.machine.clusterId,
            error,
          });
        }
      );
    } catch (error) {
      reportLateFailure(runtime.cleanupFailureReporter, {
        phase: 'engine_release',
        kind: 'cleanup',
        clusterId: runtime.machine.clusterId,
        error,
      });
      runtime.engineReleasePromise = Promise.resolve();
    }
  }
  await runtime.engineReleasePromise;
}

function waitForEngineCleanup(runtime) {
  if (typeof runtime.engineAdapter.waitForCleanup === 'function') {
    return Promise.resolve().then(() => runtime.engineAdapter.waitForCleanup());
  }
  return runtime.engineReleasePromise || runtime.engineStopPromise || Promise.resolve();
}

module.exports = { releaseEngine, stopEngine, waitForEngineCleanup };
