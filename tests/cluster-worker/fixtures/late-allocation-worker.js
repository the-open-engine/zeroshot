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
const mode = process.argv[3] || 'late';
let cluster = null;
let closeCalls = 0;
const orchestrator = {
  start() {
    if (mode === 'never') return new Promise(() => {});
    process.stderr.write('allocating\n');
    setTimeout(() => {
      cluster = { state: 'initializing' };
      process.stderr.write('allocated\n');
    }, 100);
    return new Promise(() => {});
  },
  getCluster() {
    return cluster;
  },
  stop() {
    process.stderr.write('stopping\n');
    cluster.state = 'stopped';
    fs.writeFileSync(markerPath, 'stopped\n', 'utf8');
  },
  close() {
    closeCalls += 1;
    if (mode === 'never') {
      fs.writeFileSync(markerPath, `closed:${closeCalls}\n`, 'utf8');
      return;
    }
    process.stderr.write('closed\n');
  },
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
const runtime = runClusterWorkerExecutable({ worker, shutdownMs: mode === 'never' ? 100 : 200 });
bindProcessLifecycle(runtime);
process.stderr.write('ready\n');
