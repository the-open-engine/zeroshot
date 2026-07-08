const assert = require('node:assert');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const Orchestrator = require('../../src/orchestrator');
const { registerProvider } = require('../../src/issue-providers');
const IssueProvider = require('../../src/issue-providers/base-provider');

// Minimal in-process issue provider (no network/CLI dependency) so the duplicate-run
// integration test can exercise the real input.issue path via `options.forceProvider`.
class MockIssueProvider extends IssueProvider {
  static id = 'test-mock-provider';
  static displayName = 'Test Mock Provider';

  static detectIdentifier() {
    return false;
  }

  static getRequiredTool() {
    return { name: 'mock', checkCmd: 'true', installHint: 'n/a' };
  }

  fetchIssue(identifier) {
    return Promise.resolve({
      number: Number(identifier),
      title: `Mock issue ${identifier}`,
      body: '',
      labels: [],
      comments: [],
      url: null,
      context: `Mock issue ${identifier}`,
    });
  }
}
registerProvider(MockIssueProvider);

// Single agent with no triggers: subscribes on start() but never executes a task,
// so orchestrator.start() completes without spawning a real provider CLI process.
const NOOP_CONFIG = {
  agents: [
    {
      id: 'noop',
      role: 'implementation',
      triggers: [],
      prompt: 'noop',
    },
  ],
};

describe('Orchestrator duplicate issue check', function () {
  this.timeout(5000);

  let tempDir;
  let orchestrator;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-orchestrator-dup-'));
    orchestrator = new Orchestrator({ quiet: true, skipLoad: true, storageDir: tempDir });
  });

  afterEach(async () => {
    await orchestrator.killAll();
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it('does not treat the current cluster as a duplicate of itself', () => {
    const clusterId = 'self';
    orchestrator.clusters.set(clusterId, {
      id: clusterId,
      issue: 1172,
      state: 'initializing',
      pid: process.pid,
      createdAt: Date.now(),
    });

    const active = orchestrator._getActiveClustersForIssue(1172, clusterId);
    assert.deepEqual(active, []);
  });

  it('rejects a second start() for the same issue with zero side effects', async () => {
    const clustersFilePath = path.join(tempDir, 'clusters.json');

    const first = await orchestrator.start(
      NOOP_CONFIG,
      { issue: '4242' },
      { clusterId: 'first-cluster', forceProvider: 'test-mock-provider' }
    );
    assert.strictEqual(first.state, 'running');
    assert.strictEqual(orchestrator.clusters.size, 1);

    const clustersFileBefore = fs.readFileSync(clustersFilePath, 'utf8');

    let caughtError = null;
    try {
      await orchestrator.start(
        NOOP_CONFIG,
        { issue: '4242' },
        { clusterId: 'second-cluster', forceProvider: 'test-mock-provider' }
      );
    } catch (err) {
      caughtError = err;
    }

    assert(caughtError, 'expected the duplicate start() call to reject');
    assert.strictEqual(caughtError.code, 'DUPLICATE_CLUSTER');
    assert.strictEqual(caughtError.existingClusterId, 'first-cluster');
    assert.strictEqual(caughtError.issueNumber, 4242);

    // No new cluster registered in memory - the guard rejected before allocation.
    assert.strictEqual(orchestrator.clusters.size, 1);
    assert.strictEqual(orchestrator.clusters.has('second-cluster'), false);

    // clusters.json was never touched for the rejected run.
    assert.strictEqual(fs.readFileSync(clustersFilePath, 'utf8'), clustersFileBefore);

    // No ledger/worktree allocated for the rejected clusterId.
    assert.strictEqual(fs.existsSync(path.join(tempDir, 'second-cluster.db')), false);
  });
});
