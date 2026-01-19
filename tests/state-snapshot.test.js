const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');

const Orchestrator = require('../src/orchestrator');
const Ledger = require('../src/ledger');
const MessageBus = require('../src/message-bus');
const StateSnapshotter = require('../src/state-snapshotter');
const MockTaskRunner = require('./helpers/mock-task-runner');
const {
  initStateFromIssue,
  applyPlanReady,
  applyWorkerProgress,
  applyValidationResult,
  applyInvestigationComplete,
} = require('../src/state-snapshot');

function createTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-snapshot-'));
}

function cleanupTempDir(dir) {
  if (dir && fs.existsSync(dir)) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

function buildMessage({ clusterId, topic, sender, content }) {
  return {
    id: `msg_${Math.random().toString(16).slice(2)}`,
    timestamp: Date.now(),
    cluster_id: clusterId,
    topic,
    sender,
    receiver: 'broadcast',
    content,
  };
}

async function waitForClusterState(orchestrator, clusterId, target, timeoutMs = 5000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const status = orchestrator.getStatus(clusterId);
      if (status.state === target) {
        return;
      }
    } catch {
      // Cluster may be removed during shutdown
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Cluster ${clusterId} did not reach ${target} within ${timeoutMs}ms`);
}

describe('State snapshot builder', () => {
  it('should replace plan text and criteria on PLAN_READY', () => {
    const clusterId = 'cluster-plan';
    const issueMessage = buildMessage({
      clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Fix login bug', data: { title: 'Login bug', issue_number: 133 } },
    });

    let state = initStateFromIssue(issueMessage);

    state = applyPlanReady(
      state,
      buildMessage({
        clusterId,
        topic: 'PLAN_READY',
        sender: 'planner',
        content: {
          text: 'Plan v1',
          data: {
            summary: 'Old plan',
            acceptanceCriteria: [{ id: 'AC1', criterion: 'Old criteria', priority: 'MUST' }],
            filesAffected: ['old.js'],
          },
        },
      })
    );

    state = applyPlanReady(
      state,
      buildMessage({
        clusterId,
        topic: 'PLAN_READY',
        sender: 'planner',
        content: {
          text: 'Plan v2',
          data: {
            summary: 'New plan',
            acceptanceCriteria: [{ id: 'AC1', criterion: 'New criteria', priority: 'MUST' }],
            filesAffected: ['new.js'],
          },
        },
      })
    );

    assert.strictEqual(state.plan.text, 'Plan v2');
    assert.deepStrictEqual(state.plan.acceptanceCriteria, ['AC1 (MUST): New criteria']);
    assert.deepStrictEqual(state.plan.filesAffected, ['new.js']);
  });

  it('should update progress fields from WORKER_PROGRESS', () => {
    const clusterId = 'cluster-progress';
    const issueMessage = buildMessage({
      clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Add endpoint', data: { title: 'Add endpoint' } },
    });

    let state = initStateFromIssue(issueMessage);
    state = applyWorkerProgress(
      state,
      buildMessage({
        clusterId,
        topic: 'WORKER_PROGRESS',
        sender: 'worker',
        content: {
          text: 'WIP',
          data: {
            completionStatus: {
              canValidate: false,
              percentComplete: 45,
              nextSteps: ['Write tests', 'Run lint'],
            },
          },
        },
      })
    );

    assert.strictEqual(state.progress.percentComplete, 45);
    assert.deepStrictEqual(state.progress.nextSteps, ['Write tests', 'Run lint']);
  });

  it('should update validation approval and errors', () => {
    const clusterId = 'cluster-validation';
    const issueMessage = buildMessage({
      clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Fix build', data: { title: 'Build fix' } },
    });

    let state = initStateFromIssue(issueMessage);
    state = applyValidationResult(
      state,
      buildMessage({
        clusterId,
        topic: 'VALIDATION_RESULT',
        sender: 'validator',
        content: {
          data: {
            approved: false,
            errors: ['Missing tests', 'Type error'],
          },
        },
      })
    );

    assert.strictEqual(state.validation.approved, false);
    assert.deepStrictEqual(state.validation.errors, ['Missing tests', 'Type error']);
  });

  it('should update debug fields from INVESTIGATION_COMPLETE', () => {
    const clusterId = 'cluster-debug';
    const issueMessage = buildMessage({
      clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Debug failure', data: { title: 'Debug issue' } },
    });

    let state = initStateFromIssue(issueMessage);
    state = applyInvestigationComplete(
      state,
      buildMessage({
        clusterId,
        topic: 'INVESTIGATION_COMPLETE',
        sender: 'investigator',
        content: {
          text: 'Apply guard clauses and add tests',
          data: {
            successCriteria: 'All tests pass',
            rootCauses: [{ cause: 'Null input not handled' }],
          },
        },
      })
    );

    assert.strictEqual(state.debug.fixPlan, 'Apply guard clauses and add tests');
    assert.deepStrictEqual(state.debug.rootCauses, ['Null input not handled']);
  });
});

describe('StateSnapshotter publishing', () => {
  let tempDir;
  let ledger;
  let messageBus;

  beforeEach(() => {
    tempDir = createTempDir();
    ledger = new Ledger(path.join(tempDir, 'ledger.db'));
    messageBus = new MessageBus(ledger);
  });

  afterEach(() => {
    ledger.close();
    cleanupTempDir(tempDir);
  });

  it('should publish STATE_SNAPSHOT on PLAN_READY and VALIDATION_RESULT', () => {
    const clusterId = 'cluster-publish';
    const snapshotter = new StateSnapshotter({ messageBus, clusterId });
    snapshotter.start();

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Issue text', data: { title: 'Issue title' } },
      metadata: { source: 'text' },
    });

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'PLAN_READY',
      sender: 'planner',
      content: {
        text: 'Plan text',
        data: {
          summary: 'Plan summary',
          acceptanceCriteria: [{ id: 'AC1', criterion: 'Do thing', priority: 'MUST' }],
        },
      },
    });

    const planSnapshot = messageBus.findLast({ cluster_id: clusterId, topic: 'STATE_SNAPSHOT' });
    assert.ok(planSnapshot, 'STATE_SNAPSHOT should be published');
    assert.strictEqual(planSnapshot.content.data.plan.text, 'Plan text');

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        data: {
          approved: false,
          errors: ['Test failure'],
        },
      },
    });

    const validationSnapshot = messageBus.findLast({
      cluster_id: clusterId,
      topic: 'STATE_SNAPSHOT',
    });
    assert.strictEqual(validationSnapshot.content.data.validation.approved, false);
    assert.deepStrictEqual(validationSnapshot.content.data.validation.errors, ['Test failure']);
  });
});

describe('Snapshotter orchestration integration', () => {
  let tempDir;
  let orchestrator;
  let mockRunner;

  beforeEach(() => {
    tempDir = createTempDir();
    mockRunner = new MockTaskRunner();
  });

  afterEach(() => {
    if (orchestrator) {
      orchestrator.close();
    }
    cleanupTempDir(tempDir);
  });

  it('should inject STATE_SNAPSHOT into worker context', async () => {
    mockRunner.when('worker').returns('{"summary":"done"}');

    orchestrator = new Orchestrator({
      quiet: true,
      storageDir: tempDir,
      taskRunner: mockRunner,
    });

    const config = {
      agents: [
        {
          id: 'worker',
          role: 'implementation',
          timeout: 0,
          contextStrategy: {
            sources: [
              { topic: 'STATE_SNAPSHOT', priority: 'required', strategy: 'latest', amount: 1 },
              { topic: 'ISSUE_OPENED', priority: 'required', strategy: 'latest', amount: 1 },
            ],
          },
          triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
          prompt: 'Do work',
          hooks: {
            onComplete: {
              action: 'publish_message',
              config: { topic: 'CLUSTER_COMPLETE', content: { text: 'done' } },
            },
          },
        },
        {
          id: 'completion-detector',
          role: 'orchestrator',
          timeout: 0,
          triggers: [{ topic: 'CLUSTER_COMPLETE', action: 'stop_cluster' }],
        },
      ],
    };

    const result = await orchestrator.start(config, { text: 'Do the thing' });
    await waitForClusterState(orchestrator, result.id, 'stopped', 5000);

    mockRunner.assertContextIncludes('worker', 'STATE_SNAPSHOT');
  });

  it('should publish STATE_SNAPSHOT when loading legacy clusters', async () => {
    const clusterId = 'legacy-cluster';
    const clusterDir = tempDir;
    const dbPath = path.join(clusterDir, `${clusterId}.db`);

    const ledger = new Ledger(dbPath);
    ledger.append({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Legacy issue', data: { title: 'Legacy' } },
      metadata: { source: 'text' },
    });
    ledger.append({
      cluster_id: clusterId,
      topic: 'PLAN_READY',
      sender: 'planner',
      content: { text: 'Legacy plan', data: { summary: 'Legacy summary' } },
    });
    ledger.close();

    const fixturePath = path.join(__dirname, 'fixtures', 'single-worker.json');
    const config = JSON.parse(fs.readFileSync(fixturePath, 'utf8'));
    const clustersFile = path.join(clusterDir, 'clusters.json');
    fs.writeFileSync(
      clustersFile,
      JSON.stringify(
        {
          [clusterId]: {
            id: clusterId,
            config,
            state: 'stopped',
            createdAt: Date.now(),
            pid: null,
          },
        },
        null,
        2
      )
    );

    orchestrator = await Orchestrator.create({ storageDir: clusterDir, quiet: true });
    const cluster = orchestrator.getCluster(clusterId);
    const snapshot = cluster.messageBus.findLast({
      cluster_id: clusterId,
      topic: 'STATE_SNAPSHOT',
    });

    assert.ok(snapshot, 'STATE_SNAPSHOT should be created during load');
    assert.strictEqual(snapshot.content.data.plan.text, 'Legacy plan');
  });
});
