#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const {
  createLegacyClusterWorker,
  createDeploymentProfileRegistry,
} = require('../../../lib/cluster-worker');
const { runClusterWorkerExecutable } = require('../../../lib/cluster-worker/executable');

let currentEventSink = null;

const engineAdapter = Object.freeze({
  async start({ request, clusterId, onEvent, prepareArtifacts }) {
    currentEventSink = onEvent;
    if (request.source === 'artifact') {
      await prepareArtifacts(
        Object.freeze({
          kind: 'worktree',
          hostRoot: process.cwd(),
          runtimeRoot: process.cwd(),
        })
      );
    }
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
    return { clusterId, artifactsStaged: true };
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
  artifactResolver: {
    stage(artifacts, { isolation }) {
      const artifactDirectory = path.join(isolation.hostRoot, '.openengine', 'artifacts');
      fs.mkdirSync(artifactDirectory, { recursive: true });
      const artifactPath = path.join(artifactDirectory, `${artifacts[0].artifactId}.txt`);
      fs.writeFileSync(artifactPath, 'hello', { mode: 0o400 });
      fs.chmodSync(artifactPath, 0o400);
      return {
        artifacts: [
          {
            artifactId: artifacts[0].artifactId,
            path: path.join(
              isolation.runtimeRoot,
              '.openengine',
              'artifacts',
              path.basename(artifactPath)
            ),
          },
        ],
      };
    },
  },
  engineAdapter,
  idFactory: () => 'fake-cluster-1',
});

runClusterWorkerExecutable({ worker, frameBytes: 4096, shutdownMs: 25 });
