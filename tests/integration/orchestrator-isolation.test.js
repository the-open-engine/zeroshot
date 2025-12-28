/**
 * Integration tests for Orchestrator + Isolation mode
 *
 * Verifies the REAL isolation mode flow:
 * - Workspace copied to /tmp/zeroshot-isolated/{clusterId}/
 * - Fresh git repo with feature branch zeroshot/{clusterId}
 * - Claude config with PreToolUse hook to block AskUserQuestion
 * - Container mounts correct directories
 * - Cleanup removes workspace AND container
 *
 * Uses MockTaskRunner to avoid Claude API calls while testing
 * the full Docker-based isolation integration.
 */

const assert = require('node:assert');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { execSync } = require('child_process');

const Orchestrator = require('../../src/orchestrator');
const IsolationManager = require('../../src/isolation-manager');
const MockTaskRunner = require('../helpers/mock-task-runner');

describe('Orchestrator Isolation Mode Integration', function () {
  this.timeout(180000); // Docker ops + npm install can be slow

  let orchestrator;
  let tempDir;
  let mockRunner;

  // Simple single-worker config for basic tests
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

  // Worker + Validator config for multi-agent tests
  const workerValidatorConfig = {
    agents: [
      {
        id: 'worker',
        role: 'implementation',
        timeout: 0,
        triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
        prompt: 'Implement the feature',
        hooks: {
          onComplete: {
            action: 'publish_message',
            config: { topic: 'IMPLEMENTATION_READY' },
          },
        },
      },
      {
        id: 'validator',
        role: 'validator',
        timeout: 0,
        triggers: [{ topic: 'IMPLEMENTATION_READY', action: 'execute_task' }],
        prompt: 'Validate the implementation',
        outputFormat: 'json',
        jsonSchema: {
          type: 'object',
          properties: {
            approved: { type: 'boolean' },
            summary: { type: 'string' },
          },
          required: ['approved'],
        },
        hooks: {
          onComplete: {
            action: 'publish_message',
            config: {
              topic: 'VALIDATION_RESULT',
              content: { data: { approved: '{{result.approved}}', summary: '{{result.summary}}' } },
            },
          },
        },
      },
      {
        id: 'completion-detector',
        role: 'orchestrator',
        timeout: 0,
        triggers: [
          {
            topic: 'VALIDATION_RESULT',
            action: 'stop_cluster',
            logic: {
              engine: 'javascript',
              script: `
                const result = ledger.findLast({ topic: 'VALIDATION_RESULT' });
                return result && (result.content?.data?.approved === true || result.content?.data?.approved === 'true');
              `,
            },
          },
        ],
      },
    ],
  };

  before(function () {
    // Skip isolation tests in CI (no Docker image available)
    // To run locally: docker build -t zeroshot-cluster-base docker/zeroshot-cluster/
    if (process.env.CI) {
      this.skip();
      return;
    }

    // HARD FAIL if Docker unavailable (local development)
    if (!IsolationManager.isDockerAvailable()) {
      throw new Error(
        'Docker is required for isolation tests. Ensure Docker daemon is running.'
      );
    }

    // HARD FAIL if image missing (don't auto-build in tests)
    if (!IsolationManager.imageExists('zeroshot-cluster-base')) {
      throw new Error(
        'zeroshot-cluster-base image required.\n' +
          'Build it with: docker build -t zeroshot-cluster-base external/zeroshot/docker/zeroshot-cluster/'
      );
    }
  });

  beforeEach(function () {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zs-isolation-test-'));
    mockRunner = new MockTaskRunner();
    orchestrator = new Orchestrator({
      quiet: true,
      storageDir: tempDir,
      taskRunner: mockRunner,
    });
  });

  afterEach(async function () {
    // Kill ALL clusters (even on test failure) to cleanup containers
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
  });

  describe('Container Lifecycle', function () {
    it('should create container with correct mounts and state', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test task' }, {
        isolation: true,
        cwd: process.cwd(), // Use real git repo for realistic isolation
      });

      const cluster = orchestrator.getCluster(result.id);
      assert(cluster.isolation, 'Cluster should have isolation info');
      assert(cluster.isolation.enabled, 'Isolation should be enabled');

      const containerId = cluster.isolation.containerId;
      assert(containerId, 'Container ID should exist');

      // VERIFY: Container is running
      const running = execSync(
        `docker inspect ${containerId} --format '{{.State.Running}}'`
      ).toString().trim();
      assert.strictEqual(running, 'true', 'Container should be running');

      // VERIFY: Workspace mounted at /workspace
      const wsExists = execSync(
        `docker exec ${containerId} test -d /workspace && echo yes || echo no`
      ).toString().trim();
      assert.strictEqual(wsExists, 'yes', 'Workspace should be mounted');

      // VERIFY: Git repo with feature branch
      const branch = execSync(
        `docker exec ${containerId} git branch --show-current`
      ).toString().trim();
      assert(
        branch.startsWith('zeroshot/'),
        `Branch should start with zeroshot/, got: ${branch}`
      );

      // VERIFY: Claude config with AskUserQuestion blocking hook
      const hookExists = execSync(
        `docker exec ${containerId} grep -q AskUserQuestion /home/node/.claude/settings.json && echo yes || echo no`
      ).toString().trim();
      assert.strictEqual(hookExists, 'yes', 'Claude config should have AskUserQuestion hook');

      // VERIFY: Projects directory exists (Claude CLI needs this)
      const projectsDirExists = execSync(
        `docker exec ${containerId} test -d /home/node/.claude/projects && echo yes || echo no`
      ).toString().trim();
      assert.strictEqual(projectsDirExists, 'yes', 'Claude projects directory should exist');

      await orchestrator.stop(result.id);
    });

    it('should preserve workspace on stop for resume capability', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test stop cleanup' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const clusterId = result.id;
      const cluster = orchestrator.getCluster(clusterId);
      const containerId = cluster.isolation.containerId;
      const isolatedPath = `/tmp/zeroshot-isolated/${clusterId}`;
      const configPath = `/tmp/zeroshot-cluster-configs/${clusterId}`;

      // Verify container exists before stop
      const beforeStop = execSync(
        `docker inspect ${containerId} --format '{{.State.Running}}' 2>/dev/null || echo not_found`
      ).toString().trim();
      assert.strictEqual(beforeStop, 'true', 'Container should exist before stop');

      // Stop cluster (should preserve workspace for resume)
      await orchestrator.stop(clusterId);

      // VERIFY: Container is gone (container always removed)
      try {
        execSync(`docker inspect ${containerId}`, { stdio: 'pipe' });
        assert.fail('Container should be removed after stop');
      } catch {
        // Expected - container should not exist
      }

      // VERIFY: Isolated workspace is PRESERVED (for resume capability)
      assert(
        fs.existsSync(isolatedPath),
        `Isolated workspace should be PRESERVED for resume: ${isolatedPath}`
      );

      // VERIFY: Cluster config dir is cleaned (will be recreated on resume)
      assert(
        !fs.existsSync(configPath),
        `Cluster config dir should be removed: ${configPath}`
      );
    });

    it('should fully clean up workspace and container on kill', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test kill cleanup' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const clusterId = result.id;
      const cluster = orchestrator.getCluster(clusterId);
      const containerId = cluster.isolation.containerId;
      const isolatedPath = `/tmp/zeroshot-isolated/${clusterId}`;

      // Kill cluster (full cleanup, no resume possible)
      await orchestrator.kill(clusterId);

      // VERIFY: Container is gone
      try {
        execSync(`docker inspect ${containerId}`, { stdio: 'pipe' });
        assert.fail('Container should be removed after kill');
      } catch {
        // Expected - container should not exist
      }

      // VERIFY: Isolated workspace is GONE (kill does full cleanup)
      assert(
        !fs.existsSync(isolatedPath),
        `Isolated workspace should be removed after kill: ${isolatedPath}`
      );
    });

    it('should force-remove container on kill', async function () {
      mockRunner.when('worker').delays(60000, '{"done": true}'); // Long delay

      const result = await orchestrator.start(simpleConfig, { text: 'Test kill' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const cluster = orchestrator.getCluster(result.id);
      const containerId = cluster.isolation.containerId;

      // Kill while task is still running
      await orchestrator.kill(result.id);

      // VERIFY: Container is gone (force removed)
      try {
        execSync(`docker inspect ${containerId}`, { stdio: 'pipe' });
        assert.fail('Container should be force-removed after kill');
      } catch {
        // Expected - container should not exist
      }
    });
  });

  describe('Agent Execution in Isolation', function () {
    it('should execute MockTaskRunner for agent inside container context', async function () {
      mockRunner.when('worker').returns('{"summary": "Feature implemented"}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test agent execution' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      await waitForClusterState(orchestrator, result.id, 'stopped', 60000);

      // VERIFY: MockTaskRunner was called
      mockRunner.assertCalled('worker', 1);

      // VERIFY: Context included the task
      const calls = mockRunner.getCalls('worker');
      assert(
        calls[0].context.includes('Test agent execution'),
        'Context should include task text'
      );

      // VERIFY: Message published to ledger
      const cluster = orchestrator.getCluster(result.id);
      const msgs = cluster.messageBus.query({
        cluster_id: result.id,
        topic: 'TASK_COMPLETE',
      });
      assert(msgs.length > 0, 'TASK_COMPLETE should be published');
    });

    it('should run worker-validator flow with consensus in isolation', async function () {
      mockRunner.when('worker').returns('{"summary": "Implemented the feature"}');
      mockRunner.when('validator').returns('{"approved": true, "summary": "LGTM"}');

      const result = await orchestrator.start(workerValidatorConfig, { text: 'Test multi-agent' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      await waitForClusterState(orchestrator, result.id, 'stopped', 60000);

      // VERIFY: Both agents were called
      mockRunner.assertCalled('worker', 1);
      mockRunner.assertCalled('validator', 1);

      // VERIFY: Validation result in ledger
      const cluster = orchestrator.getCluster(result.id);
      const validations = cluster.messageBus.query({
        cluster_id: result.id,
        topic: 'VALIDATION_RESULT',
      });
      assert(validations.length > 0, 'VALIDATION_RESULT should be published');
      assert.strictEqual(
        validations[0].content.data.approved,
        true,
        'Validator should approve'
      );
    });

    it('should publish messages in correct order in isolation', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test message order' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      await waitForClusterState(orchestrator, result.id, 'stopped', 60000);

      const cluster = orchestrator.getCluster(result.id);
      const allMsgs = cluster.messageBus.getAll(result.id);

      // Get topic sequence (filter out lifecycle messages)
      const workflowTopics = allMsgs
        .filter((m) => ['ISSUE_OPENED', 'TASK_COMPLETE'].includes(m.topic))
        .map((m) => m.topic);

      // VERIFY: Correct order
      assert.strictEqual(workflowTopics[0], 'ISSUE_OPENED', 'First should be ISSUE_OPENED');
      assert(
        workflowTopics.includes('TASK_COMPLETE'),
        'Should include TASK_COMPLETE'
      );
    });
  });

  describe('Resume Capability', function () {
    // Config that doesn't auto-complete (no completion-detector)
    const resumeTestConfig = {
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
        // NO completion-detector - cluster won't auto-stop
      ],
    };

    it('should recreate container on resume using preserved workspace', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      // Start cluster in isolation mode
      const result = await orchestrator.start(resumeTestConfig, { text: 'Test resume' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const clusterId = result.id;
      const cluster = orchestrator.getCluster(clusterId);
      const oldContainerId = cluster.isolation.containerId;
      const isolatedPath = `/tmp/zeroshot-isolated/${clusterId}`;

      // Wait for first task to complete
      await new Promise((r) => setTimeout(r, 2000));

      // Manually stop (preserves workspace)
      await orchestrator.stop(clusterId);

      // VERIFY: Container gone but workspace preserved
      try {
        execSync(`docker inspect ${oldContainerId}`, { stdio: 'pipe' });
        assert.fail('Old container should be removed after stop');
      } catch {
        // Expected
      }
      assert(fs.existsSync(isolatedPath), 'Workspace should be preserved for resume');

      // VERIFY: workDir was saved in cluster state
      const stoppedCluster = orchestrator.getCluster(clusterId);
      assert(stoppedCluster.isolation.workDir, 'workDir should be saved in isolation state');

      // Now resume - should recreate container using preserved workspace
      mockRunner.when('worker').returns('{"summary": "Resumed work"}');
      await orchestrator.resume(clusterId, 'Continue the work');

      // Get new container ID
      const resumedCluster = orchestrator.getCluster(clusterId);
      const newContainerId = resumedCluster.isolation.containerId;

      // VERIFY: New container was created
      assert(newContainerId, 'New container should be created on resume');
      assert.notStrictEqual(
        newContainerId,
        oldContainerId,
        'New container ID should differ from old'
      );

      // VERIFY: New container is running
      const running = execSync(
        `docker inspect ${newContainerId} --format '{{.State.Running}}'`
      ).toString().trim();
      assert.strictEqual(running, 'true', 'New container should be running');

      // VERIFY: Workspace mounted (same preserved workspace)
      const wsExists = execSync(
        `docker exec ${newContainerId} test -d /workspace && echo yes || echo no`
      ).toString().trim();
      assert.strictEqual(wsExists, 'yes', 'Workspace should be mounted');

      // VERIFY: Git branch preserved (work not lost)
      const branch = execSync(
        `docker exec ${newContainerId} git branch --show-current`
      ).toString().trim();
      assert(branch.startsWith('zeroshot/'), 'Git branch should be preserved');

      await orchestrator.kill(clusterId);
    });

    it('should update all agents with new container ID on resume', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(resumeTestConfig, { text: 'Test agent update' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const clusterId = result.id;
      await new Promise((r) => setTimeout(r, 2000));
      await orchestrator.stop(clusterId);

      // Resume
      mockRunner.when('worker').returns('{"summary": "Resumed"}');
      await orchestrator.resume(clusterId);

      // Get resumed cluster
      const cluster = orchestrator.getCluster(clusterId);
      const newContainerId = cluster.isolation.containerId;

      // VERIFY: All agents have updated isolation context
      for (const agent of cluster.agents) {
        if (agent.isolation?.enabled) {
          // Agent should have access to container manager
          assert(
            agent.isolation.manager,
            `Agent ${agent.id} should have isolation manager`
          );
        }
      }

      // VERIFY: Cluster isolation manager has container registered
      assert(
        cluster.isolation.manager.containers.get(clusterId) === newContainerId,
        'Manager should have new container registered'
      );

      await orchestrator.kill(clusterId);
    });

    it('should not be resumable after kill (cluster removed)', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(resumeTestConfig, { text: 'Test fail' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const clusterId = result.id;
      const isolatedPath = `/tmp/zeroshot-isolated/${clusterId}`;
      await new Promise((r) => setTimeout(r, 2000));

      // VERIFY: Workspace exists before kill
      assert(fs.existsSync(isolatedPath), 'Workspace should exist before kill');

      // KILL (not stop) - this deletes workspace AND removes cluster from disk
      await orchestrator.kill(clusterId);

      // VERIFY: Workspace is deleted
      assert(!fs.existsSync(isolatedPath), 'Workspace should be deleted after kill');

      // VERIFY: Cluster is removed from memory
      assert(!orchestrator.getCluster(clusterId), 'Cluster should be removed from memory');

      // Recreate orchestrator to simulate fresh process loading from disk
      const newOrchestrator = new Orchestrator({
        quiet: true,
        storageDir: tempDir,
        taskRunner: mockRunner,
      });

      // VERIFY: Cluster not found (killed clusters are not persisted)
      assert(
        !newOrchestrator.getCluster(clusterId),
        'Killed cluster should not be loadable'
      );

      // Try to resume - should fail because cluster doesn't exist
      try {
        await newOrchestrator.resume(clusterId);
        assert.fail('Resume should fail for killed cluster');
      } catch (err) {
        assert(
          err.message.includes('not found'),
          `Error should indicate cluster not found: ${err.message}`
        );
      }
    });
  });

  describe('Git Isolation', function () {
    it('should create fresh git repo with single initial commit', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test git isolation' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const cluster = orchestrator.getCluster(result.id);
      const containerId = cluster.isolation.containerId;

      // VERIFY: Git log shows exactly one commit
      const commitCount = execSync(
        `docker exec ${containerId} git rev-list --count HEAD`
      ).toString().trim();
      assert.strictEqual(commitCount, '1', 'Should have exactly one initial commit');

      // VERIFY: Commit message indicates isolated copy
      const commitMsg = execSync(
        `docker exec ${containerId} git log -1 --format=%s`
      ).toString().trim();
      assert(
        commitMsg.includes('Initial commit') || commitMsg.includes('isolated'),
        `Commit message should indicate isolation: ${commitMsg}`
      );

      await orchestrator.stop(result.id);
    });

    it('should create feature branch with clusterId', async function () {
      mockRunner.when('worker').returns('{"done": true}');

      const result = await orchestrator.start(simpleConfig, { text: 'Test branch' }, {
        isolation: true,
        cwd: process.cwd(),
      });

      const cluster = orchestrator.getCluster(result.id);
      const containerId = cluster.isolation.containerId;

      // VERIFY: Branch name includes cluster ID
      const branch = execSync(
        `docker exec ${containerId} git branch --show-current`
      ).toString().trim();

      // Extract cluster suffix from full ID (e.g., cluster-cosmic-meteor-87 -> cosmic-meteor-87)
      const clusterSuffix = result.id.replace(/^cluster-/, '');
      assert(
        branch.includes(clusterSuffix),
        `Branch ${branch} should include cluster suffix ${clusterSuffix}`
      );

      await orchestrator.stop(result.id);
    });
  });
});

/**
 * Wait for cluster to reach a specific state
 */
async function waitForClusterState(orchestrator, clusterId, targetState, timeoutMs) {
  const startTime = Date.now();

  while (Date.now() - startTime < timeoutMs) {
    const cluster = orchestrator.getCluster(clusterId);
    if (!cluster) {
      throw new Error(`Cluster ${clusterId} not found`);
    }

    if (cluster.state === targetState) {
      return;
    }

    await new Promise((r) => setTimeout(r, 100));
  }

  const cluster = orchestrator.getCluster(clusterId);
  throw new Error(
    `Timeout waiting for cluster ${clusterId} to reach state '${targetState}'. ` +
    `Current state: ${cluster?.state}`
  );
}
