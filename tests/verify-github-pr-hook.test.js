/**
 * verify_github_pr Hook Action Test Suite
 *
 * Tests for the verify_github_pr hook action that validates PR existence and merge status
 * Part of issue #340 - Prevent git-pusher hallucination
 */

const assert = require("assert");
const path = require("path");

// Mock agent with required methods
function createMockAgent(workingDirectory = process.cwd()) {
  return {
    id: "test-agent",
    role: "test",
    workingDirectory,
    _log: () => {},
    _publish: function (message) {
      this.lastPublished = message;
    },
    lastPublished: null,
  };
}

describe("verify_github_pr hook action", function () {
  this.timeout(10000);

  let executeHook;
  let mockExecSyncFn;

  beforeEach(() => {
    // Clear module cache
    const hookExecutorPath = path.join(__dirname, "../src/agent/agent-hook-executor.js");
    delete require.cache[require.resolve(hookExecutorPath)];

    const safeExecPath = path.join(__dirname, "../src/lib/safe-exec.js");
    delete require.cache[require.resolve(safeExecPath)];

    // Mock safe-exec module
    require.cache[require.resolve(safeExecPath)] = {
      exports: {
        execSync: function (...args) {
          if (mockExecSyncFn) {
            return mockExecSyncFn(...args);
          }
          throw new Error("Mock execSync not configured");
        },
      },
    };

    // Reload executeHook with mocked safe-exec
    executeHook = require("../src/agent/agent-hook-executor").executeHook;
    mockExecSyncFn = null;
  });

  afterEach(() => {
    mockExecSyncFn = null;
  });

  it("should throw when pr_number missing from output", async function () {
    const agent = createMockAgent();
    const hook = { action: "verify_github_pr" };
    const result = {
      output: JSON.stringify({
        pr_url: "https://github.com/org/repo/pull/123",
        merged: true,
      }),
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail("Expected error to be thrown");
    } catch (err) {
      assert.match(err.message, /VERIFICATION FAILED.*pr_number/i);
    }
  });

  it("should throw when PR does not exist in GitHub", async function () {
    const agent = createMockAgent();
    const hook = { action: "verify_github_pr" };
    const result = {
      output: JSON.stringify({
        pr_number: 9999,
        pr_url: "https://github.com/org/repo/pull/9999",
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      const error = new Error("Could not resolve to a PullRequest");
      error.status = 1;
      throw error;
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail("Expected error to be thrown");
    } catch (err) {
      assert.match(err.message, /DOES NOT EXIST/);
      assert.match(err.message, /HALLUCINATED/);
    }
  });

  it("should throw when PR exists but not merged", async function () {
    const agent = createMockAgent();
    const hook = { action: "verify_github_pr" };
    const result = {
      output: JSON.stringify({
        pr_number: 123,
        pr_url: "https://github.com/org/repo/pull/123",
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 123,
        state: "OPEN",
        mergedAt: null,
        url: "https://github.com/org/repo/pull/123",
      });
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail("Expected error to be thrown");
    } catch (err) {
      assert.match(err.message, /not merged|LIED/i);
    }
  });

  it("should publish CLUSTER_COMPLETE when PR verified merged", async function () {
    const agent = createMockAgent();
    const hook = { action: "verify_github_pr" };
    const result = {
      output: JSON.stringify({
        pr_number: 456,
        pr_url: "https://github.com/org/repo/pull/456",
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      return JSON.stringify({
        number: 456,
        state: "MERGED",
        mergedAt: "2026-01-15T10:30:00Z",
        url: "https://github.com/org/repo/pull/456",
      });
    };

    await executeHook({ hook, agent, result });

    assert(agent.lastPublished, "Expected message to be published");
    assert.strictEqual(agent.lastPublished.topic, "CLUSTER_COMPLETE");
    assert.strictEqual(agent.lastPublished.content.data.pr_number, 456);
  });

  it("should pass correct workingDirectory to gh CLI", async function () {
    const agent = createMockAgent("/custom/work/dir");
    const hook = { action: "verify_github_pr" };
    const result = {
      output: JSON.stringify({
        pr_number: 789,
        pr_url: "https://github.com/org/repo/pull/789",
        merged: true,
      }),
    };

    let capturedCwd;
    mockExecSyncFn = (cmd, opts) => {
      capturedCwd = opts.cwd;
      return JSON.stringify({
        number: 789,
        state: "MERGED",
        mergedAt: "2026-01-15T10:30:00Z",
        url: "https://github.com/org/repo/pull/789",
      });
    };

    await executeHook({ hook, agent, result });
    assert.strictEqual(capturedCwd, "/custom/work/dir");
  });

  it("should propagate non-hallucination errors", async function () {
    const agent = createMockAgent();
    const hook = { action: "verify_github_pr" };
    const result = {
      output: JSON.stringify({
        pr_number: 999,
        pr_url: "https://github.com/org/repo/pull/999",
        merged: true,
      }),
    };

    mockExecSyncFn = () => {
      throw new Error("Network error: timeout");
    };

    try {
      await executeHook({ hook, agent, result });
      assert.fail("Expected error to be thrown");
    } catch (err) {
      assert.match(err.message, /Network error: timeout/);
    }
  });
});
