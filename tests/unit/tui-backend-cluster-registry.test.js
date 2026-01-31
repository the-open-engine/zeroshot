const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui-backend',
  'services',
  'cluster-registry.js'
);
const sourcePath = path.join(
  __dirname,
  '..',
  '..',
  'src',
  'tui-backend',
  'services',
  'cluster-registry.ts'
);

function ensureBackendBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui-backend', { stdio: 'inherit' });
    return;
  }
  if (fs.existsSync(sourcePath)) {
    const buildMtime = fs.statSync(buildOutput).mtimeMs;
    const sourceMtime = fs.statSync(sourcePath).mtimeMs;
    if (sourceMtime > buildMtime) {
      execSync('npm run build:tui-backend', { stdio: 'inherit' });
    }
  }
}

ensureBackendBuild();

const {
  listClusters,
  getClusterSummary,
  ClusterNotFoundError,
} = require('../../lib/tui-backend/services/cluster-registry');

function createOrchestrator(summaries, clustersById) {
  return {
    listClusters() {
      return summaries;
    },
    getCluster(id) {
      return clustersById[id];
    },
  };
}

describe('tui-backend cluster registry', function () {
  it('resolves provider with forceProvider -> defaultProvider -> settings default', async function () {
    const summaries = [
      { id: 'cluster-force', state: 'running', createdAt: 1, agentCount: 1, messageCount: 1 },
      { id: 'cluster-default', state: 'running', createdAt: 2, agentCount: 1, messageCount: 1 },
      { id: 'cluster-settings', state: 'running', createdAt: 3, agentCount: 1, messageCount: 1 },
      { id: 'cluster-empty', state: 'running', createdAt: 4, agentCount: 1, messageCount: 1 },
    ];
    const clustersById = {
      'cluster-force': { config: { forceProvider: 'openai', defaultProvider: 'claude' } },
      'cluster-default': { config: { defaultProvider: 'google' } },
      'cluster-settings': { config: {} },
      'cluster-empty': {},
    };
    const orchestrator = createOrchestrator(summaries, clustersById);

    const result = await listClusters({
      deps: {
        getOrchestrator: () => Promise.resolve(orchestrator),
        loadSettings: () => ({ defaultProvider: 'opencode' }),
      },
    });

    const providerById = Object.fromEntries(
      result.map((cluster) => [cluster.id, cluster.provider])
    );
    assert.strictEqual(providerById['cluster-force'], 'codex');
    assert.strictEqual(providerById['cluster-default'], 'gemini');
    assert.strictEqual(providerById['cluster-settings'], 'opencode');
    assert.strictEqual(providerById['cluster-empty'], 'opencode');
  });

  it('throws ClusterNotFoundError for missing cluster id', async function () {
    const summaries = [
      { id: 'cluster-1', state: 'running', createdAt: 1, agentCount: 1, messageCount: 1 },
    ];
    const clustersById = {
      'cluster-1': { config: { defaultProvider: 'claude' } },
    };
    const orchestrator = createOrchestrator(summaries, clustersById);

    try {
      await getClusterSummary({
        clusterId: 'missing-cluster',
        deps: {
          getOrchestrator: () => Promise.resolve(orchestrator),
          loadSettings: () => ({ defaultProvider: 'claude' }),
        },
      });
      assert.fail('Expected ClusterNotFoundError');
    } catch (error) {
      assert.ok(error instanceof ClusterNotFoundError);
      assert.strictEqual(error.clusterId, 'missing-cluster');
    }
  });
});
