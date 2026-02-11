/**
 * verify_github_pr Hook Action Test Suite
 *
 * Tests for the verify_github_pr hook action that validates PR existence and merge status
 * Part of issue #340 - Prevent git-pusher hallucination
 */

const assert = require('assert');
const path = require('path');

// Mock agent with required methods
function createMockAgent(workingDirectory = process.cwd()) {
  return {
    id: 'test-agent',
    role: 'test',
    workingDirectory,
    _log: () => {},
    _publish: function (message) {
      this.lastPublished = message;
    },
    lastPublished: null,
  };
}

describe('verify_github_pr hook action', function () {
  this.timeout(60000);

  let executeHook;
  let mockExecSyncFn;

  beforeEach(() => {
    // Clear module cache
    const hookExecutorPath = path.join(__dirname, '../src/agent/agent-hook-executor.js');
    delete require.cache[require.resolve(hookExecutorPath)];

    const safeExecPath = path.join(__dirname, '../src/lib/safe-exec.js');
    delete require.cache[require.resolve(safeExecPath)];

    // Mock safe-exec module
    require.cache[require.resolve(safeExecPath)] = {
      exports: {
        execSync: function (...args) {
          if (mockExecSyncFn) {
            return mockExecSyncFn(...args);
          }
          throw new Error('Mock execSync not configured');
        },
      },
    };

    // Reload executeHook with mocked safe-exec
    executeHook = require('../src/agent/agent-hook-executor').executeHook;
    mockExecSyncFn = null;
  });

  afterEach(() => {
    mockExecSyncFn = null;
  });

  it('should not require pr_number in structured output', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        summary: 'Merged',
        result:
          'PR merged: {"pr_url":"https://github.com/org/repo/pull/123","pr_number":123,"merged":true}',
      }),
    };

    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 123,
        state: 'MERGED',
        mergedAt: '2026-01-15T10:30:00Z',
        url: 'https://github.com/org/repo/pull/123',
      });
    };

    await executeHook({ hook, agent, result });
    assert(agent.lastPublished, 'Expected message to be published');
    assert.strictEqual(agent.lastPublished.topic, 'CLUSTER_COMPLETE');
  });

  it('should throw when PR does not exist in GitHub', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/9999',
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      const error = new Error('Could not resolve to a PullRequest');
      error.status = 1;
      throw error;
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail('Expected error to be thrown');
    } catch (err) {
      assert.match(err.message, /DOES NOT EXIST/);
      assert.match(err.message, /HALLUCINATED/);
    }
  });

  it('should throw when PR exists but genuinely not merged after all polls', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/123',
        pr_number: 123,
        merged: true,
      }),
    };

    // Always returns OPEN — genuinely not merged
    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 123,
        state: 'OPEN',
        mergedAt: null,
        url: 'https://github.com/org/repo/pull/123',
      });
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail('Expected error to be thrown');
    } catch (err) {
      assert.match(err.message, /LIED/i);
      assert.match(err.message, /polls/i);
    }
  });

  // REGRESSION: gentle-hydra-56 (2026-02-11)
  // GitHub API returned state="OPEN" immediately after gh pr merge, but PR was actually merged.
  // Old code had no merge propagation polling — killed the cluster after 3s.
  it('should succeed when GitHub API shows OPEN initially then MERGED after propagation delay', async function () {
    const agent = createMockAgent();
    agent._log = () => {}; // suppress log noise
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/1411',
        pr_number: 1411,
        merged: true,
      }),
    };

    // Simulate GitHub eventual consistency: OPEN for first 3 calls, then MERGED
    let callCount = 0;
    mockExecSyncFn = () => {
      callCount++;
      if (callCount <= 3) {
        return JSON.stringify({
          number: 1411,
          state: 'OPEN',
          mergedAt: null,
          url: 'https://github.com/org/repo/pull/1411',
        });
      }
      return JSON.stringify({
        number: 1411,
        state: 'MERGED',
        mergedAt: '2026-02-11T10:08:37Z',
        url: 'https://github.com/org/repo/pull/1411',
      });
    };

    await executeHook({ hook, agent, result });

    assert(agent.lastPublished, 'Expected CLUSTER_COMPLETE to be published');
    assert.strictEqual(agent.lastPublished.topic, 'CLUSTER_COMPLETE');
    assert.strictEqual(agent.lastPublished.content.data.pr_number, 1411);
    assert(callCount >= 4, `Expected at least 4 gh calls (got ${callCount})`);
  });

  it('should use explicit PR number in gh command when available in agent output', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/555',
        pr_number: 555,
        merged: true,
      }),
    };

    let capturedCmd;
    mockExecSyncFn = (cmd) => {
      capturedCmd = cmd;
      return JSON.stringify({
        number: 555,
        state: 'MERGED',
        mergedAt: '2026-02-11T10:00:00Z',
        url: 'https://github.com/org/repo/pull/555',
      });
    };

    await executeHook({ hook, agent, result });
    assert(capturedCmd.includes('gh pr view 555'), `Expected PR number in command, got: ${capturedCmd}`);
  });

  it('should fall back to branch-based resolution when pr_number not in output', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        summary: 'Done',
      }),
    };

    let capturedCmd;
    mockExecSyncFn = (cmd) => {
      capturedCmd = cmd;
      return JSON.stringify({
        number: 100,
        state: 'MERGED',
        mergedAt: '2026-02-11T10:00:00Z',
        url: 'https://github.com/org/repo/pull/100',
      });
    };

    await executeHook({ hook, agent, result });
    assert.strictEqual(capturedCmd, 'gh pr view --json state,mergedAt,url,number');
  });

  it('should publish CLUSTER_COMPLETE when PR verified merged', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/456',
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 456,
        state: 'MERGED',
        mergedAt: '2026-01-15T10:30:00Z',
        url: 'https://github.com/org/repo/pull/456',
      });
    };

    await executeHook({ hook, agent, result });

    assert(agent.lastPublished, 'Expected message to be published');
    assert.strictEqual(agent.lastPublished.topic, 'CLUSTER_COMPLETE');
    assert.strictEqual(agent.lastPublished.content.data.pr_number, 456);
  });

  it('should pass correct workingDirectory to gh CLI', async function () {
    const agent = createMockAgent('/custom/work/dir');
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/789',
        merged: true,
      }),
    };

    let capturedCwd;
    mockExecSyncFn = (cmd, opts) => {
      capturedCwd = opts.cwd;
      return JSON.stringify({
        number: 789,
        state: 'MERGED',
        mergedAt: '2026-01-15T10:30:00Z',
        url: 'https://github.com/org/repo/pull/789',
      });
    };

    await executeHook({ hook, agent, result });
    assert.strictEqual(capturedCwd, '/custom/work/dir');
  });

  it('should propagate non-hallucination errors', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/999',
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      throw new Error('Network error: timeout');
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail('Expected error to be thrown');
    } catch (err) {
      assert.match(err.message, /Network error: timeout/);
    }
  });

  it('should throw when claimed pr_url does not match the branch PR', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_github_pr' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/111',
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 222,
        state: 'MERGED',
        mergedAt: '2026-01-15T10:30:00Z',
        url: 'https://github.com/org/repo/pull/222',
      });
    };

    await assert.rejects(() => executeHook({ hook, agent, result }), /claimed PR URL/i);
  });
});
