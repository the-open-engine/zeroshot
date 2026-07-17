#!/usr/bin/env node
'use strict';

const {
  createLegacyClusterWorker,
  createDeploymentProfileRegistry,
} = require('../../../lib/cluster-worker');
const { runClusterWorkerExecutable } = require('../../../lib/cluster-worker/executable');

let currentEventSink = null;

const engineAdapter = Object.freeze({
  start({ request, clusterId, onEvent }) {
    currentEventSink = onEvent;
    const scenario = request.source === 'prompt' ? request.prompt : request.source;
    if (scenario === 'success' || scenario === 'artifact') {
      setImmediate(() => onEvent({ type: 'complete', summary: `${scenario} complete` }));
    } else if (scenario === 'failure') {
      setImmediate(() => onEvent({ type: 'failed', code: 'crash', reason: 'declared_failure' }));
    } else if (scenario === 'malformed') {
      setImmediate(() =>
        onEvent({
          type: 'complete',
          result: { summary: 'bad', status: 'succeeded', artifacts: [{ bytes: 'inline' }] },
        })
      );
    }
    return { clusterId };
  },
  status() {
    return { state: currentEventSink ? 'running' : 'idle' };
  },
  stop() {
    return { effective: true };
  },
});

const worker = createLegacyClusterWorker({
  profileRegistry: createDeploymentProfileRegistry({
    isolationProfiles: {
      'isolation.worktree@1': { worktree: true },
      'isolation.none@1': {},
    },
    bounds: { executionMs: 25, shutdownMs: 25, frameBytes: 4096 },
  }),
  engineAdapter,
  idFactory: () => 'fake-cluster-1',
});

runClusterWorkerExecutable({ worker, frameBytes: 4096, shutdownMs: 25 });
