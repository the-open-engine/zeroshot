const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const Orchestrator = require('../../src/orchestrator');

function createTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-load-isolation-'));
}

function cleanupTempDir(dirPath) {
  fs.rmSync(dirPath, { recursive: true, force: true });
}

describe('Orchestrator load isolation', function () {
  let storageDir;

  beforeEach(function () {
    storageDir = createTempDir();
  });

  afterEach(function () {
    cleanupTempDir(storageDir);
  });

  it('keeps loading healthy clusters when one persisted cluster fails to reload', async function () {
    fs.writeFileSync(
      path.join(storageDir, 'clusters.json'),
      JSON.stringify(
        {
          'broken-cluster': { id: 'broken-cluster', state: 'stopped' },
          'healthy-cluster': { id: 'healthy-cluster', state: 'running' },
        },
        null,
        2
      )
    );
    fs.writeFileSync(path.join(storageDir, 'broken-cluster.db'), '');
    fs.writeFileSync(path.join(storageDir, 'healthy-cluster.db'), '');

    const orchestrator = new Orchestrator({
      storageDir,
      skipLoad: true,
      quiet: true,
    });

    orchestrator._loadSingleCluster = function loadSingleCluster(clusterId, clusterData) {
      if (clusterId === 'broken-cluster') {
        throw new Error('SQLite runtime unavailable for live cluster execution.');
      }

      const cluster = {
        id: clusterId,
        ...clusterData,
        messageBus: {
          count() {
            return 1;
          },
        },
      };
      this.clusters.set(clusterId, cluster);
      return cluster;
    };

    await orchestrator._loadClusters();

    assert.deepStrictEqual([...orchestrator.clusters.keys()], ['healthy-cluster']);
  });
});
