#!/usr/bin/env node
'use strict';

const path = require('node:path');
const {
  bindProcessLifecycle,
  redirectConsoleToStderr,
} = require('../../../lib/cluster-worker/process-stdio');

redirectConsoleToStderr();

const {
  createLegacyClusterWorker,
  createDeploymentProfileRegistry,
} = require('../../../lib/cluster-worker');
const { runClusterWorkerExecutable } = require('../../../lib/cluster-worker/executable');

const successConfig = path.join(__dirname, 'single-worker-config.json');
const failureConfig = path.join(__dirname, 'cluster-worker-failure-config.json');
const malformedConfig = path.join(__dirname, 'cluster-worker-malformed-config.json');
const providerSettings = { defaultProvider: 'claude', providerSettings: {} };
const profileRegistry = createDeploymentProfileRegistry({
  isolationProfiles: {
    'isolation.worktree@1': { worktree: true, cwd: process.cwd() },
  },
  providerProfiles: {
    'provider.fake@1': {
      configPath: successConfig,
      providerOverride: 'claude',
      settings: providerSettings,
    },
    'provider.fake.failure@1': {
      configPath: failureConfig,
      providerOverride: 'claude',
      settings: providerSettings,
    },
    'provider.fake.malformed@1': {
      configPath: malformedConfig,
      providerOverride: 'claude',
      settings: providerSettings,
    },
    'provider.fake.timeout@1': {
      configPath: successConfig,
      providerOverride: 'claude',
      settings: providerSettings,
      bounds: { executionMs: 1000 },
    },
  },
  bounds: { executionMs: 30000, shutdownMs: 2000, frameBytes: 65536 },
});

const worker = createLegacyClusterWorker({
  profileRegistry,
  idFactory: () => 'fake-provider-cluster',
});
const runtime = runClusterWorkerExecutable({ worker });
bindProcessLifecycle(runtime);
