'use strict';

const fs = require('fs');
const { createLegacyClusterWorker } = require('../../../lib/cluster-worker');
const { createCurrentEngineAdapter } = require('../../../lib/cluster-worker/engine-adapter');
const { runClusterWorkerExecutable } = require('../../../lib/cluster-worker/executable');
const {
  bindProcessLifecycle,
  redirectConsoleToStderr,
} = require('../../../lib/cluster-worker/process-stdio');
const { createDeploymentProfileRegistry } = require('../../../lib/cluster-worker/profiles');

redirectConsoleToStderr();

const markerPath = process.argv[2];
let cluster = null;
const orchestrator = {
  start() {
    setTimeout(() => {
      cluster = { state: 'initializing' };
    }, 100);
    return new Promise(() => {});
  },
  getCluster() {
    return cluster;
  },
  stop() {
    cluster.state = 'stopped';
    fs.writeFileSync(markerPath, 'stopped\n', 'utf8');
  },
  close() {},
};
const startCluster = {
  prepareClusterConfig(config) {
    return config;
  },
  buildTrustedStartOptions(value) {
    return { clusterId: value.clusterId, worktree: true };
  },
};
const profileRegistry = createDeploymentProfileRegistry({
  bounds: { executionMs: 1000, shutdownMs: 10 },
  providerProfiles: {
    'provider.default@1': { config: { agents: [] }, settings: {} },
  },
});
const engineAdapter = createCurrentEngineAdapter({ orchestrator, startCluster });
const worker = createLegacyClusterWorker({
  profileRegistry,
  engineAdapter,
  idFactory: () => 'late-allocation-cluster',
});
const runtime = runClusterWorkerExecutable({ worker, shutdownMs: 10 });
bindProcessLifecycle(runtime);
process.stderr.write('ready\n');
