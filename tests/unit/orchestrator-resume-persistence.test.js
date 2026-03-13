const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const Orchestrator = require('../../src/orchestrator');
const MockTaskRunner = require('../helpers/mock-task-runner');

function createTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-resume-persistence-'));
}

function cleanupTempDir(tmpDir) {
  fs.rmSync(tmpDir, { recursive: true, force: true });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function createSingleWorkerConfig() {
  return {
    agents: [
      {
        id: 'worker',
        role: 'implementation',
        modelLevel: 'level2',
        outputFormat: 'text',
        triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
        prompt: 'You are a worker agent. Implement the requested task.',
      },
    ],
  };
}

describe('Orchestrator resume persistence', function () {
  this.timeout(15000);

  let storageDir;
  let mockRunner;

  beforeEach(function () {
    storageDir = createTempDir();
    mockRunner = new MockTaskRunner();
  });

  afterEach(function () {
    cleanupTempDir(storageDir);
  });

  it('persists running ownership and lifecycle state when resuming a loaded cluster', async function () {
    const orchestrator = new Orchestrator({
      taskRunner: mockRunner,
      storageDir,
      skipLoad: true,
      quiet: true,
    });

    mockRunner.when('worker').delays(250, { done: true });

    const result = await orchestrator.start(createSingleWorkerConfig(), { text: 'Task' });
    const clusterId = result.id;

    await sleep(25);
    await orchestrator.stop(clusterId);
    orchestrator.close();

    const resumedOrchestrator = await Orchestrator.create({
      taskRunner: mockRunner,
      storageDir,
      quiet: true,
    });

    try {
      mockRunner.when('worker').delays(250, { done: true, resumed: true });

      await resumedOrchestrator.resume(clusterId);

      const clustersFile = path.join(storageDir, 'clusters.json');
      const start = Date.now();
      let persisted = null;

      while (Date.now() - start < 3000) {
        persisted = JSON.parse(fs.readFileSync(clustersFile, 'utf8'));
        const clusterState = persisted[clusterId];
        const workerState = clusterState?.agentStates?.find((agent) => agent.id === 'worker');

        if (
          clusterState?.state === 'running' &&
          clusterState?.pid === process.pid &&
          workerState?.state === 'executing_task'
        ) {
          break;
        }

        await sleep(25);
      }

      assert.ok(persisted?.[clusterId], 'Resumed cluster should remain persisted');
      assert.strictEqual(persisted[clusterId].state, 'running');
      assert.strictEqual(persisted[clusterId].pid, process.pid);

      const resumedWorker = persisted[clusterId].agentStates.find((agent) => agent.id === 'worker');
      assert.strictEqual(resumedWorker?.state, 'executing_task');
    } finally {
      await resumedOrchestrator.stop(clusterId);
      resumedOrchestrator.close();
    }
  });

  it('re-registers completion handlers when clusters are loaded from disk', async function () {
    const orchestrator = new Orchestrator({
      taskRunner: mockRunner,
      storageDir,
      skipLoad: true,
      quiet: true,
    });

    mockRunner.when('worker').delays(250, { done: true });

    const result = await orchestrator.start(createSingleWorkerConfig(), { text: 'Task' });
    const clusterId = result.id;

    await sleep(25);
    await orchestrator.stop(clusterId);
    orchestrator.close();

    const resumedOrchestrator = await Orchestrator.create({
      taskRunner: mockRunner,
      storageDir,
      quiet: true,
    });

    try {
      let stopCall = null;
      const originalStop = resumedOrchestrator.stop.bind(resumedOrchestrator);
      resumedOrchestrator.stop = async (id, options = {}) => {
        stopCall = { id, options };
        return originalStop(id, options);
      };

      const loadedCluster = resumedOrchestrator.getCluster(clusterId);
      loadedCluster.messageBus.publish({
        cluster_id: clusterId,
        topic: 'CLUSTER_COMPLETE',
        sender: 'tester',
        content: {
          data: { reason: 'resume-regression-test' },
        },
      });

      const start = Date.now();
      while (Date.now() - start < 3000 && !stopCall) {
        await sleep(25);
      }

      assert.ok(stopCall, 'Loaded cluster should react to CLUSTER_COMPLETE');
      assert.strictEqual(stopCall.id, clusterId);
      assert.strictEqual(stopCall.options.completedSuccessfully, true);
    } finally {
      if (!resumedOrchestrator.getCluster(clusterId)?.stopped) {
        await resumedOrchestrator.stop(clusterId);
      }
      resumedOrchestrator.close();
    }
  });
});
