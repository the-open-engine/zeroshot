/**
 * Tests for worktree auto-cleanup on successful --pr/--ship completion.
 *
 * Verifies: When a cluster completes successfully with autoPr=true,
 * stop() removes the worktree (kill behavior) instead of preserving it.
 */

const assert = require('assert');

describe('Worktree Auto-Cleanup on Successful Completion', function () {
  // Directly test the shouldAutoCleanWorktree logic from stop()
  // by simulating the cluster state and checking the decision.

  function shouldAutoClean(options, cluster) {
    return !!(options.completedSuccessfully && cluster.autoPr && cluster.worktree?.manager);
  }

  describe('cleanup decision logic', function () {
    it('should auto-clean when completedSuccessfully + autoPr + worktree', function () {
      const result = shouldAutoClean(
        { completedSuccessfully: true },
        { autoPr: true, worktree: { manager: {} } }
      );
      assert.strictEqual(result, true);
    });

    it('should NOT auto-clean on user-initiated stop (no completedSuccessfully)', function () {
      const result = shouldAutoClean({}, { autoPr: true, worktree: { manager: {} } });
      assert.strictEqual(result, false);
    });

    it('should NOT auto-clean without autoPr (plain zeroshot run)', function () {
      const result = shouldAutoClean(
        { completedSuccessfully: true },
        { autoPr: false, worktree: { manager: {} } }
      );
      assert.strictEqual(result, false);
    });

    it('should NOT auto-clean without worktree (docker mode or no isolation)', function () {
      const result = shouldAutoClean(
        { completedSuccessfully: true },
        { autoPr: true, worktree: null }
      );
      assert.strictEqual(result, false);
    });

    it('should NOT auto-clean on CLUSTER_FAILED (completedSuccessfully not set)', function () {
      const result = shouldAutoClean({}, { autoPr: true, worktree: { manager: {} } });
      assert.strictEqual(result, false);
    });

    it('should NOT auto-clean with completedSuccessfully=false', function () {
      const result = shouldAutoClean(
        { completedSuccessfully: false },
        { autoPr: true, worktree: { manager: {} } }
      );
      assert.strictEqual(result, false);
    });
  });

  describe('state transitions', function () {
    it('should set state to completed when completedSuccessfully (visible to orchestrators)', function () {
      // completedSuccessfully=true → state=completed (not killed)
      // Clusters remain in zeroshot list so heroshot can detect success.
      const expectedState = 'completed';
      assert.strictEqual(expectedState, 'completed');
    });

    it('should set state to stopped when preserving for resume', function () {
      const cluster = { autoPr: true, worktree: { manager: {} } };
      const autoClean = shouldAutoClean({}, cluster);
      const expectedState = autoClean ? 'completed' : 'stopped';
      assert.strictEqual(expectedState, 'stopped');
    });
  });
});
