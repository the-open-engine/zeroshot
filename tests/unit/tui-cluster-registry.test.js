const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const buildOutput = path.join(
  __dirname,
  '..',
  '..',
  'lib',
  'tui',
  'services',
  'cluster-registry.js'
);

function ensureTuiBuild() {
  if (!fs.existsSync(buildOutput)) {
    execSync('npm run build:tui', { stdio: 'inherit' });
  }
}

ensureTuiBuild();

const { listClusters } = require('../../lib/tui/services/cluster-registry');

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

describe('TUI cluster registry', function () {
  it('sorts clusters by createdAt ascending and then id', async function () {
    const summaries = [
      { id: 'cluster-b', state: 'running', createdAt: 200, agentCount: 1, messageCount: 2 },
      { id: 'cluster-a', state: 'completed', createdAt: 100, agentCount: 2, messageCount: 5 },
      { id: 'cluster-c', state: 'running', createdAt: 100, agentCount: 1, messageCount: 1 },
    ];
    const clustersById = {
      'cluster-a': { worktree: { path: '/tmp/a' } },
      'cluster-b': { isolation: { workDir: '/tmp/b' } },
      'cluster-c': {},
    };
    const orchestrator = createOrchestrator(summaries, clustersById);

    const result = await listClusters({
      deps: { getOrchestrator: () => Promise.resolve(orchestrator) },
    });

    assert.deepStrictEqual(
      result.map((cluster) => cluster.id),
      ['cluster-a', 'cluster-c', 'cluster-b']
    );
  });

  it('derives cwd from worktree then isolation workDir', async function () {
    const summaries = [
      { id: 'cluster-1', state: 'running', createdAt: 1, agentCount: 1, messageCount: 1 },
      { id: 'cluster-2', state: 'running', createdAt: 2, agentCount: 1, messageCount: 1 },
      { id: 'cluster-3', state: 'running', createdAt: 3, agentCount: 1, messageCount: 1 },
    ];
    const clustersById = {
      'cluster-1': { worktree: { path: '/worktree/path' }, isolation: { workDir: '/iso' } },
      'cluster-2': { isolation: { workDir: '/isolation/path' } },
      'cluster-3': {},
    };
    const orchestrator = createOrchestrator(summaries, clustersById);

    const result = await listClusters({
      deps: { getOrchestrator: () => Promise.resolve(orchestrator) },
    });

    const cwdById = Object.fromEntries(result.map((cluster) => [cluster.id, cluster.cwd]));
    assert.strictEqual(cwdById['cluster-1'], '/worktree/path');
    assert.strictEqual(cwdById['cluster-2'], '/isolation/path');
    assert.strictEqual(cwdById['cluster-3'], null);
  });
});
