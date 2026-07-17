'use strict';

const { releaseEngine, waitForEngineCleanup } = require('./runtime-engine');

const RELEASE_WORKER = Symbol('releaseLegacyClusterWorker');
const WAIT_FOR_WORKER_CLEANUP = Symbol('waitForLegacyClusterWorkerCleanup');

function createWorkerFacade(runtime) {
  return Object.freeze({
    start: runtime.start.bind(runtime),
    status: runtime.status.bind(runtime),
    events: runtime.events.bind(runtime),
    stop: runtime.stop.bind(runtime),
    result: runtime.result.bind(runtime),
    [RELEASE_WORKER]: () => releaseEngine(runtime),
    [WAIT_FOR_WORKER_CLEANUP]: () => waitForEngineCleanup(runtime),
  });
}

module.exports = { RELEASE_WORKER, WAIT_FOR_WORKER_CLEANUP, createWorkerFacade };
