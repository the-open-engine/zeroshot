const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { spawn } = require('node:child_process');

const Orchestrator = require('../../src/orchestrator');

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForClusterRecord(storageDir, clusterId, expectedPid, timeoutMs = 10000) {
  const clustersFile = path.join(storageDir, 'clusters.json');
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    if (fs.existsSync(clustersFile)) {
      try {
        const data = JSON.parse(fs.readFileSync(clustersFile, 'utf8'));
        const record = data[clusterId];
        if (record && record.state === 'running' && record.pid) {
          if (expectedPid) {
            assert.strictEqual(
              record.pid,
              expectedPid,
              `Expected cluster pid ${expectedPid}, got ${record.pid}`
            );
          }
          return record;
        }
      } catch {
        // Ignore transient parse errors while file is being written.
      }
    }
    await sleep(100);
  }

  throw new Error(`Timed out waiting for cluster ${clusterId} in ${clustersFile}`);
}

function waitForChildExit(child, timeoutMs = 10000) {
  if (child.exitCode !== null) {
    return child.exitCode;
  }

  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`Timed out waiting for child process ${child.pid} to exit`));
    }, timeoutMs);

    child.once('exit', (code) => {
      clearTimeout(timer);
      resolve(code);
    });

    child.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

describe('Detached daemon stop', function () {
  this.timeout(20000);

  let tempDir;
  let child;

  afterEach(function () {
    if (child && child.exitCode === null) {
      try {
        process.kill(child.pid, 'SIGKILL');
      } catch {
        // Ignore cleanup errors
      }
    }

    if (tempDir && fs.existsSync(tempDir)) {
      try {
        fs.rmSync(tempDir, { recursive: true, force: true });
      } catch {
        // Ignore cleanup errors
      }
    }
  });

  it('signals remote daemon pid and halts ledger activity', async function () {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-detached-stop-'));
    const clusterId = `detached-stop-${Date.now()}`;
    const fixturePath = path.join(__dirname, '..', 'fixtures', 'detached-daemon.js');

    child = spawn(process.execPath, [fixturePath], {
      env: {
        ...process.env,
        ZEROSHOT_TEST_STORAGE: tempDir,
        ZEROSHOT_TEST_CLUSTER_ID: clusterId,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    await waitForClusterRecord(tempDir, clusterId, child.pid);
    await sleep(200);

    const orchestrator = await Orchestrator.create({
      quiet: true,
      storageDir: tempDir,
    });

    const cluster = orchestrator.getCluster(clusterId);
    assert(cluster, 'Cluster should be loaded from storage');

    const beforeCount = cluster.messageBus.count({ cluster_id: clusterId });

    await orchestrator.stop(clusterId);
    await waitForChildExit(child, 10000);

    const afterStopCount = cluster.messageBus.count({ cluster_id: clusterId });
    await sleep(250);
    const afterWaitCount = cluster.messageBus.count({ cluster_id: clusterId });

    assert.strictEqual(
      afterStopCount,
      afterWaitCount,
      'No new ledger messages should appear after stop'
    );

    const status = orchestrator.getStatus(clusterId);
    assert.strictEqual(status.state, 'stopped');
    assert.strictEqual(status.pid, null);
    assert.strictEqual(beforeCount, afterStopCount);

    orchestrator.close();
  });
});
