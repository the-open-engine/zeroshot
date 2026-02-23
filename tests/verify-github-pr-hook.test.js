/**
 * verify_pull_request Hook Action Test Suite
 *
 * Tests for the verify_pull_request hook action that validates PR existence and merge status
 * Part of issue #340 - Prevent git-pusher hallucination
 */

/* global describe, beforeEach, afterEach, it */
const assert = require('assert');

// Mock agent with required methods
function createMockAgent(workingDirectory = process.cwd(), providerName = 'claude') {
  return {
    id: 'test-agent',
    role: 'test',
    workingDirectory,
    _resolveProvider: () => providerName,
    _log: () => {},
    _publish: function (message) {
      this.lastPublished = message;
    },
    lastPublished: null,
  };
}

describe('verify_pull_request hook action', () => {
  let executeHook;
  let mockExecSyncFn;
  let mockPlatformResolver;
  let previousPollAttempts;
  let previousPollIntervalMs;

  beforeEach(() => {
    previousPollAttempts = process.env.ZEROSHOT_PR_MERGE_POLL_ATTEMPTS;
    previousPollIntervalMs = process.env.ZEROSHOT_PR_MERGE_POLL_INTERVAL_MS;
    process.env.ZEROSHOT_PR_MERGE_POLL_ATTEMPTS = '4';
    process.env.ZEROSHOT_PR_MERGE_POLL_INTERVAL_MS = '1';

    // Clear module cache for ALL modules in the dependency chain
    const safeExecPath = require.resolve('../src/lib/safe-exec.js');
    const prVerificationPath = require.resolve('../src/agent/pr-verification.js');
    const hookTemplatePath = require.resolve('../src/agent/hook-template.js');
    const hookTransformPath = require.resolve('../src/agent/hook-transform.js');
    const hookLogicPath = require.resolve('../src/agent/hook-logic.js');
    const hookSandboxPath = require.resolve('../src/agent/hook-sandbox.js');
    const issueProvidersPath = require.resolve('../src/issue-providers/index.js');
    const hookExecutorPath = require.resolve('../src/agent/agent-hook-executor.js');

    delete require.cache[hookExecutorPath];
    delete require.cache[prVerificationPath];
    delete require.cache[hookTemplatePath];
    delete require.cache[hookTransformPath];
    delete require.cache[hookLogicPath];
    delete require.cache[hookSandboxPath];
    delete require.cache[issueProvidersPath];
    delete require.cache[safeExecPath];

    // Mock safe-exec module BEFORE reloading any modules that depend on it
    require.cache[safeExecPath] = {
      id: safeExecPath,
      filename: safeExecPath,
      loaded: true,
      exports: {
        execSync: function (...args) {
          if (mockExecSyncFn) {
            return mockExecSyncFn(...args);
          }
          throw new Error('Mock execSync not configured');
        },
      },
    };

    mockPlatformResolver = null;
    require.cache[issueProvidersPath] = {
      id: issueProvidersPath,
      filename: issueProvidersPath,
      loaded: true,
      exports: {
        getPlatformForPR: (cwd) => (mockPlatformResolver ? mockPlatformResolver(cwd) : 'github'),
      },
    };

    // Reload executeHook — pr-verification.js will pick up mocked safe-exec
    executeHook = require('../src/agent/agent-hook-executor').executeHook;
    mockExecSyncFn = null;
  });

  afterEach(() => {
    mockExecSyncFn = null;
    mockPlatformResolver = null;
    if (previousPollAttempts === undefined) {
      delete process.env.ZEROSHOT_PR_MERGE_POLL_ATTEMPTS;
    } else {
      process.env.ZEROSHOT_PR_MERGE_POLL_ATTEMPTS = previousPollAttempts;
    }
    if (previousPollIntervalMs === undefined) {
      delete process.env.ZEROSHOT_PR_MERGE_POLL_INTERVAL_MS;
    } else {
      process.env.ZEROSHOT_PR_MERGE_POLL_INTERVAL_MS = previousPollIntervalMs;
    }
  });

  it('should verify PR when pr_url present but pr_number missing', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        summary: 'Merged',
        pr_url: 'https://github.com/org/repo/pull/123',
        merged: true,
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
    const hook = { action: 'verify_pull_request' };
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

  it('should complete with verification-pending when PR remains OPEN after all polls', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/123',
        pr_number: 123,
        merged: true,
      }),
    };

    // Always returns OPEN
    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 123,
        state: 'OPEN',
        mergedAt: null,
        url: 'https://github.com/org/repo/pull/123',
      });
    };

    await executeHook({ hook, agent, result });
    assert(agent.lastPublished, 'Expected message to be published');
    assert.strictEqual(agent.lastPublished.topic, 'CLUSTER_COMPLETE');
    assert.strictEqual(
      agent.lastPublished.content.data.reason,
      'git-pusher-complete-verification-pending'
    );
    assert.strictEqual(agent.lastPublished.content.data.verification_pending, true);
  });

  it('should throw when PR is CLOSED without merge after all polls', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/123',
        pr_number: 123,
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 123,
        state: 'CLOSED',
        mergedAt: null,
        url: 'https://github.com/org/repo/pull/123',
      });
    };

    await assert.rejects(
      () => executeHook({ hook, agent, result }),
      /exists but is not merged \(state="CLOSED"\)/i
    );
  });

  // REGRESSION: gentle-hydra-56 (2026-02-11)
  // GitHub API returned state="OPEN" immediately after gh pr merge, but PR was actually merged.
  // Old code had no merge propagation polling — killed the cluster after 3s.
  it('should succeed when GitHub API shows OPEN initially then MERGED after propagation delay', async function () {
    const agent = createMockAgent();
    agent._log = () => {}; // suppress log noise
    const hook = { action: 'verify_pull_request' };
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
    const hook = { action: 'verify_pull_request' };
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
    assert(
      capturedCmd.includes('gh pr view 555'),
      `Expected PR number in command, got: ${capturedCmd}`
    );
  });

  // REGRESSION: flying-jungle-51 (2026-02-16)
  // Agent failed to create PR (type errors blocked commit). Structured output had no pr_number/pr_url.
  // Old code fell through to `gh pr view` which found an unrelated open PR → "Agent LIED" error.
  it('should throw when structured output has no PR data (agent failed to create PR)', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        summary: 'PR creation blocked - TypeScript compilation errors',
        result: 'Failed to create PR due to pre-commit errors',
      }),
    };

    // Should NOT reach gh pr view at all
    mockExecSyncFn = () => {
      assert.fail('gh pr view should not be called when no PR data in output');
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail('Expected error to be thrown');
    } catch (err) {
      assert.match(err.message, /without creating a PR/);
      assert.match(err.message, /no pr_number, mr_number, pr_url, or mr_url/i);
      assert.match(err.message, /compilation errors/i);
    }
  });

  // REGRESSION: provider mismatch in hook parser
  // verify_pull_request previously parsed Codex output with Claude parser assumptions.
  // That dropped pr_number/pr_url even when the assistant output contained valid JSON.
  it('should parse Codex output with provider-aware extraction', async function () {
    const agent = createMockAgent(process.cwd(), 'codex');
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: [
        JSON.stringify({
          type: 'item.created',
          item: {
            type: 'message',
            role: 'assistant',
            content: [{ type: 'text', text: '{"pr_num' }],
          },
        }),
        JSON.stringify({
          type: 'item.created',
          item: {
            type: 'message',
            role: 'assistant',
            content: [{ type: 'text', text: 'ber":321,"merged":true}' }],
          },
        }),
      ].join('\n'),
    };

    let capturedCmd;
    mockExecSyncFn = (cmd) => {
      capturedCmd = cmd;
      return JSON.stringify({
        number: 321,
        state: 'MERGED',
        mergedAt: '2026-02-17T10:00:00Z',
        url: 'https://github.com/org/repo/pull/321',
      });
    };

    await executeHook({ hook, agent, result });
    assert(
      capturedCmd.includes('gh pr view 321'),
      `Expected PR number in command, got: ${capturedCmd}`
    );
    assert.strictEqual(agent.lastPublished.content.data.pr_number, 321);
  });

  // REGRESSION: hook must recover PR metadata from raw command output when
  // structured extraction misses fields.
  it('should recover PR metadata from raw output fallback extraction', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        type: 'item.completed',
        item: {
          type: 'command_execution',
          aggregated_output: 'Created pull request https://github.com/org/repo/pull/654',
          exit_code: 0,
        },
      }),
    };

    let capturedCmd;
    mockExecSyncFn = (cmd) => {
      capturedCmd = cmd;
      return JSON.stringify({
        number: 654,
        state: 'MERGED',
        mergedAt: '2026-02-17T10:00:00Z',
        url: 'https://github.com/org/repo/pull/654',
      });
    };

    await executeHook({ hook, agent, result });
    assert(
      capturedCmd.includes('gh pr view 654'),
      `Expected PR number in command, got: ${capturedCmd}`
    );
    assert.strictEqual(agent.lastPublished.content.data.pr_number, 654);
  });

  it('should derive PR number from pr_url when pr_number is missing', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        pr_url: 'https://github.com/org/repo/pull/100',
        merged: true,
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
    assert.strictEqual(capturedCmd, 'gh pr view 100 --json state,mergedAt,url,number');
  });

  it('should publish CLUSTER_COMPLETE when PR verified merged', async function () {
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
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
    const hook = { action: 'verify_pull_request' };
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
    const hook = { action: 'verify_pull_request' };
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
    const hook = { action: 'verify_pull_request' };
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

    await assert.rejects(() => executeHook({ hook, agent, result }), /claimed URL/i);
  });

  it('should verify GitLab merge requests when platform is gitlab', async function () {
    mockPlatformResolver = () => 'gitlab';
    const agent = createMockAgent();
    const hook = { action: 'verify_pull_request' };
    const result = {
      output: JSON.stringify({
        mr_url: 'https://gitlab.com/org/repo/-/merge_requests/42',
        mr_number: 42,
        merged: true,
      }),
    };

    let capturedCmd;
    mockExecSyncFn = (cmd) => {
      capturedCmd = cmd;
      return JSON.stringify({
        iid: 42,
        state: 'merged',
        merged_at: '2026-02-23T12:00:00Z',
        web_url: 'https://gitlab.com/org/repo/-/merge_requests/42',
      });
    };

    await executeHook({ hook, agent, result });
    assert.strictEqual(capturedCmd, 'glab mr view 42 --output json');
    assert.strictEqual(agent.lastPublished.content.data.pr_number, 42);
    assert.strictEqual(agent.lastPublished.content.data.mr_number, 42);
    assert.strictEqual(agent.lastPublished.content.data.verification_platform, 'gitlab');
  });
});
