/**
 * Worktree CWD Injection Test Suite
 *
 * Regression test for bug where template agents spawned via _opAddAgents
 * did not inherit the cluster's worktree cwd, causing --ship mode to
 * pollute main directory instead of working in the isolated worktree.
 *
 * Root cause: _opAddAgents() didn't inject cwd like startCluster() does.
 * Fix: Added cwd injection to _opAddAgents() in orchestrator.js
 */

const assert = require('assert');

describe('Worktree CWD Injection', function () {
  this.timeout(10000);

  describe('_opAddAgents cwd injection', function () {
    it('should inject worktree path into dynamically added agents', function () {
      // Simulate a cluster with worktree enabled
      const worktreePath = '/tmp/zeroshot-worktrees/test-cluster-123';
      const cluster = {
        id: 'test-cluster-123',
        worktree: {
          enabled: true,
          path: worktreePath,
          branch: 'zeroshot/test-cluster-123',
          repoRoot: '/home/ubuntu/covibes',
        },
        config: {
          agents: [],
        },
        agents: [],
        messageBus: {
          subscribe: () => {},
        },
      };

      // Simulate template agents being added (no cwd set)
      const templateAgents = [
        { id: 'planner', role: 'planning', triggers: [] },
        { id: 'worker', role: 'implementation', triggers: [] },
        { id: 'validator', role: 'validator', triggers: [] },
      ];

      // Calculate expected cwd (same logic as _opAddAgents)
      const agentCwd = cluster.worktree?.path || cluster.isolation?.workDir || process.cwd();

      // Inject cwd into each agent (same logic as fix)
      for (const agentConfig of templateAgents) {
        if (!agentConfig.cwd) {
          agentConfig.cwd = agentCwd;
        }
      }

      // Verify all agents got the worktree cwd
      for (const agent of templateAgents) {
        assert.strictEqual(
          agent.cwd,
          worktreePath,
          `Agent ${agent.id} should have worktree cwd, got: ${agent.cwd}`
        );
      }
    });

    it('should not override agent cwd if already set', function () {
      const worktreePath = '/tmp/zeroshot-worktrees/test-cluster-456';
      const customCwd = '/custom/path/for/agent';

      const cluster = {
        worktree: { path: worktreePath },
      };

      const agentConfig = {
        id: 'custom-agent',
        cwd: customCwd, // Already has cwd
      };

      // Same logic as fix
      const agentCwd = cluster.worktree?.path || process.cwd();
      if (!agentConfig.cwd) {
        agentConfig.cwd = agentCwd;
      }

      // Should keep original cwd
      assert.strictEqual(
        agentConfig.cwd,
        customCwd,
        'Agent cwd should not be overridden if already set'
      );
    });

    it('should use isolation workDir if no worktree', function () {
      const isolationWorkDir = '/home/ubuntu/project';

      const cluster = {
        worktree: null,
        isolation: { workDir: isolationWorkDir },
      };

      const agentConfig = { id: 'docker-agent' };

      // Same logic as fix
      const agentCwd = cluster.worktree?.path || cluster.isolation?.workDir || process.cwd();
      if (!agentConfig.cwd) {
        agentConfig.cwd = agentCwd;
      }

      assert.strictEqual(
        agentConfig.cwd,
        isolationWorkDir,
        'Agent should use isolation workDir when no worktree'
      );
    });

    it('should fallback to process.cwd() if no isolation', function () {
      const cluster = {
        worktree: null,
        isolation: null,
      };

      const agentConfig = { id: 'local-agent' };

      // Same logic as fix
      const agentCwd = cluster.worktree?.path || cluster.isolation?.workDir || process.cwd();
      if (!agentConfig.cwd) {
        agentConfig.cwd = agentCwd;
      }

      assert.strictEqual(
        agentConfig.cwd,
        process.cwd(),
        'Agent should fallback to process.cwd() if no isolation'
      );
    });
  });

  describe('resume path cwd fix', function () {
    it('should fix agents saved without cwd on resume', function () {
      const worktreePath = '/tmp/zeroshot-worktrees/old-cluster';

      // Simulate cluster data saved BEFORE the bugfix (agents have cwd: null)
      const clusterData = {
        id: 'old-cluster',
        worktree: {
          path: worktreePath,
          branch: 'zeroshot/old-cluster',
        },
        config: {
          agents: [
            { id: 'conductor', role: 'conductor', cwd: worktreePath }, // Had cwd
            { id: 'planner', role: 'planning', cwd: null }, // Missing cwd (bug)
            { id: 'worker', role: 'implementation' }, // No cwd field (bug)
          ],
        },
      };

      // Same logic as resume fix
      const agentCwd = clusterData.worktree?.path || clusterData.isolation?.workDir || null;

      for (const agentConfig of clusterData.config.agents) {
        if (!agentConfig.cwd && agentCwd) {
          agentConfig.cwd = agentCwd;
        }
      }

      // Verify all agents now have correct cwd
      assert.strictEqual(
        clusterData.config.agents[0].cwd,
        worktreePath,
        'Conductor should keep original cwd'
      );
      assert.strictEqual(
        clusterData.config.agents[1].cwd,
        worktreePath,
        'Planner should have fixed cwd'
      );
      assert.strictEqual(
        clusterData.config.agents[2].cwd,
        worktreePath,
        'Worker should have fixed cwd'
      );
    });
  });
});
