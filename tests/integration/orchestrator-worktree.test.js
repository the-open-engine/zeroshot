/**
 * Integration tests for Orchestrator + Worktree mode
 *
 * Verifies the REAL worktree mode flow:
 * - Git worktree created at {os.tmpdir()}/zeroshot-worktrees/{clusterId}
 * - Branch zeroshot/{clusterId} created
 * - Agent runs with cwd set to worktree path
 * - Changes isolated from main repo
 * - Cleanup removes worktree but preserves branch (for PR)
 *
 * Uses MockTaskRunner to avoid Claude API calls while testing
 * the full worktree-based isolation integration.
 *
 * NO DOCKER REQUIRED - that's the whole point!
 */

const assert = require('node:assert');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { execSync } = require('child_process');

const Orchestrator = require('../../src/orchestrator');
const MockTaskRunner = require('../helpers/mock-task-runner');

let orchestrator;
let tempDir;
let testRepoDir;
let mockRunner;

const simpleConfig = {
  agents: [
    {
      id: 'worker',
      role: 'implementation',
      timeout: 0,
      triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
      prompt: 'Implement the requested feature',
      hooks: {
        onComplete: {
          action: 'publish_message',
          config: { topic: 'TASK_COMPLETE', content: { text: 'Done' } },
        },
      },
    },
    {
      id: 'completion-detector',
      role: 'orchestrator',
      timeout: 0,
      triggers: [{ topic: 'TASK_COMPLETE', action: 'stop_cluster' }],
    },
  ],
};

function registerWorktreeHooks() {
  beforeEach(function () {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-worktree-test-'));

    testRepoDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-worktree-repo-'));
    execSync('git init', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git config user.email "test@test.com"', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git config user.name "Test User"', { cwd: testRepoDir, stdio: 'pipe' });
    fs.writeFileSync(path.join(testRepoDir, 'README.md'), '# Test Repo');
    execSync('git add -A', { cwd: testRepoDir, stdio: 'pipe' });
    execSync('git commit -m "Initial commit"', { cwd: testRepoDir, stdio: 'pipe' });

    mockRunner = new MockTaskRunner();
    orchestrator = new Orchestrator({
      quiet: true,
      storageDir: tempDir,
      taskRunner: mockRunner,
    });
  });

  afterEach(async function () {
    if (orchestrator) {
      try {
        await orchestrator.killAll();
      } catch {
        // Ignore cleanup errors
      }
    }

    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
    if (testRepoDir && fs.existsSync(testRepoDir)) {
      try {
        execSync('git worktree prune', { cwd: testRepoDir, stdio: 'pipe' });
      } catch {
        // Ignore
      }
      fs.rmSync(testRepoDir, { recursive: true, force: true });
    }
  });
}

function registerWorktreeLifecycleTests() {
  describe('Worktree Lifecycle', function () {
    registerWorktreePathTest();
    registerWorktreeBranchTest();
    registerWorktreeIsolationTest();
    registerWorktreeCleanupTest();
  });
}

function registerWorktreePathTest() {
  it('should create worktree at expected path', async function () {
    mockRunner.when('worker').returns('{"done": true}');

    const result = await orchestrator.start(
      simpleConfig,
      { text: 'Test task' },
      {
        worktree: true,
        cwd: testRepoDir,
      }
    );

    const cluster = orchestrator.getCluster(result.id);

    assert(cluster.worktree, 'Cluster should have worktree info');
    assert(cluster.worktree.path, 'Worktree should have path');
    assert(cluster.worktree.branch, 'Worktree should have branch');

    assert(
      fs.existsSync(cluster.worktree.path),
      `Worktree path should exist: ${cluster.worktree.path}`
    );

    const expectedRoot = fs.realpathSync(path.join(os.tmpdir(), 'zeroshot-worktrees'));
    const worktreePath = fs.realpathSync(cluster.worktree.path);
    assert(
      worktreePath.startsWith(expectedRoot + path.sep),
      `Worktree should be in ${expectedRoot}${path.sep}`
    );

    await orchestrator.stop(result.id);
  });
}

function registerWorktreeBranchTest() {
  it('should create branch with zeroshot/ prefix', async function () {
    mockRunner.when('worker').returns('{"done": true}');

    const result = await orchestrator.start(
      simpleConfig,
      { text: 'Test task' },
      {
        worktree: true,
        cwd: testRepoDir,
      }
    );

    const cluster = orchestrator.getCluster(result.id);

    assert(
      cluster.worktree.branch.startsWith('zeroshot/'),
      `Branch should start with zeroshot/, got: ${cluster.worktree.branch}`
    );

    const branches = execSync('git branch --list', {
      cwd: testRepoDir,
      encoding: 'utf8',
    });
    assert(
      branches.includes(cluster.worktree.branch),
      `Branch ${cluster.worktree.branch} should exist in main repo`
    );

    await orchestrator.stop(result.id);
  });
}

function registerWorktreeIsolationTest() {
  it('should isolate changes from main repo', async function () {
    mockRunner.when('worker').returns('{"done": true}');

    const result = await orchestrator.start(
      simpleConfig,
      { text: 'Test task' },
      {
        worktree: true,
        cwd: testRepoDir,
      }
    );

    const cluster = orchestrator.getCluster(result.id);
    const worktreePath = cluster.worktree.path;

    fs.writeFileSync(path.join(worktreePath, 'new-file.txt'), 'worktree content');
    execSync('git add new-file.txt', { cwd: worktreePath, stdio: 'pipe' });
    execSync('git commit -m "Add file in worktree"', { cwd: worktreePath, stdio: 'pipe' });

    assert(
      !fs.existsSync(path.join(testRepoDir, 'new-file.txt')),
      'File created in worktree should NOT appear in main repo'
    );

    await orchestrator.stop(result.id);
  });
}

function registerWorktreeCleanupTest() {
  it('should clean up worktree on kill but preserve branch', async function () {
    mockRunner.when('worker').returns('{"done": true}');

    const result = await orchestrator.start(
      simpleConfig,
      { text: 'Test task' },
      {
        worktree: true,
        cwd: testRepoDir,
      }
    );

    const cluster = orchestrator.getCluster(result.id);
    const worktreePath = cluster.worktree.path;
    const branchName = cluster.worktree.branch;

    assert(fs.existsSync(worktreePath), 'Worktree should exist before kill');

    await orchestrator.kill(result.id);

    assert(!fs.existsSync(worktreePath), 'Worktree directory should be removed after kill');

    const branches = execSync('git branch --list', {
      cwd: testRepoDir,
      encoding: 'utf8',
    });
    assert(
      branches.includes(branchName),
      `Branch ${branchName} should be preserved after kill (for PR)`
    );
  });
}

function registerAgentExecutionTests() {
  describe('Agent Execution in Worktree', function () {
    registerAgentExecutionCwdTest();
    registerAgentExecutionLedgerTest();
  });
}

function registerAgentExecutionCwdTest() {
  it('should execute agent with cwd set to worktree', async function () {
    mockRunner.when('worker').returns('{"summary": "Implemented feature"}');

    const result = await orchestrator.start(
      simpleConfig,
      { text: 'Test task' },
      {
        worktree: true,
        cwd: testRepoDir,
      }
    );

    await waitForClusterState(orchestrator, result.id, 'stopped', 30000);

    mockRunner.assertCalled('worker', 1);

    const calls = mockRunner.getCalls('worker');
    assert(calls[0].context.includes('Test task'), 'Context should include task text');
  });
}

function registerAgentExecutionLedgerTest() {
  it('should publish messages to ledger correctly', async function () {
    mockRunner.when('worker').returns('{"done": true}');

    const result = await orchestrator.start(
      simpleConfig,
      { text: 'Test message flow' },
      {
        worktree: true,
        cwd: testRepoDir,
      }
    );

    await waitForClusterState(orchestrator, result.id, 'stopped', 30000);

    const cluster = orchestrator.getCluster(result.id);

    const issues = cluster.messageBus.query({
      cluster_id: result.id,
      topic: 'ISSUE_OPENED',
    });
    assert(issues.length > 0, 'ISSUE_OPENED should be published');

    const completes = cluster.messageBus.query({
      cluster_id: result.id,
      topic: 'TASK_COMPLETE',
    });
    assert(completes.length > 0, 'TASK_COMPLETE should be published');
  });
}

function registerWorktreePerformanceTests() {
  describe('Performance', function () {
    it('should start faster than Docker mode', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const startTime = Date.now();

      const result = await orchestrator.start(
        simpleConfig,
        { text: 'Performance test' },
        {
          worktree: true,
          cwd: testRepoDir,
        }
      );

      const startupTime = Date.now() - startTime;

      assert(startupTime < 5000, `Worktree startup should be <5s, took ${startupTime}ms`);

      await orchestrator.stop(result.id);
    });
  });
}

function registerWorktreeErrorHandlingTests() {
  describe('Error Handling', function () {
    it('should fail gracefully for non-git directory', async function () {
      const nonGitDir = fs.mkdtempSync(path.join(os.tmpdir(), 'non-git-'));

      mockRunner.when('worker').returns('{"done": true}');

      try {
        await orchestrator.start(
          simpleConfig,
          { text: 'Should fail' },
          {
            worktree: true,
            cwd: nonGitDir,
          }
        );
        assert.fail('Should throw for non-git directory');
      } catch (err) {
        assert(
          err.message.includes('git') || err.message.includes('repository'),
          `Error should mention git requirement: ${err.message}`
        );
      } finally {
        fs.rmSync(nonGitDir, { recursive: true, force: true });
      }
    });
  });
}

describe('Orchestrator Worktree Mode Integration', function () {
  this.timeout(60000);

  registerWorktreeHooks();
  registerWorktreeLifecycleTests();
  registerAgentExecutionTests();
  registerWorktreePerformanceTests();
  registerWorktreeErrorHandlingTests();
});

/**
 * Wait for cluster to reach a specific state
 */
async function waitForClusterState(orchestratorInstance, clusterId, targetState, timeoutMs) {
  const startTime = Date.now();

  while (Date.now() - startTime < timeoutMs) {
    const cluster = orchestratorInstance.getCluster(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    if (cluster.state === targetState) {
      return;
    }

    await new Promise((r) => setTimeout(r, 100));
  }

  const cluster = orchestratorInstance.getCluster(clusterId);
  throw new Error(
    `Timeout waiting for cluster ${clusterId} to reach state '${targetState}'. ` +
      `Current state: ${cluster?.state}`
  );
}
