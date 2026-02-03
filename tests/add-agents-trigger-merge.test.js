/**
 * Test: add_agents should REPLACE agents with duplicate IDs entirely
 *
 * HISTORY:
 * - Original behavior: Merged triggers but kept old hooks → BUG
 * - Bug manifestation: heavy-validation consensus-coordinator used quick-validation's
 *   hooks, publishing QUICK_VALIDATION_PASSED instead of VALIDATION_RESULT → infinite loop
 * - Fix: Replace agent entirely when same ID encountered
 *
 * DESIGN DECISION: Same ID = same agent = full replacement
 * If you need different behavior, use different agent IDs.
 */

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const Orchestrator = require('../src/orchestrator.js');
const MockTaskRunner = require('./helpers/mock-task-runner.js');

// Isolate tests from user settings
const testSettingsDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-settings-'));
const testSettingsFile = path.join(testSettingsDir, 'settings.json');
fs.writeFileSync(testSettingsFile, JSON.stringify({ maxModel: 'opus', minModel: null }));
process.env.ZEROSHOT_SETTINGS_FILE = testSettingsFile;

function createTempDir() {
  const tmpBase = path.join(os.tmpdir(), 'zeroshot-test');
  if (!fs.existsSync(tmpBase)) {
    fs.mkdirSync(tmpBase, { recursive: true });
  }
  return fs.mkdtempSync(path.join(tmpBase, 'agent-replace-'));
}

function cleanupTempDir(tmpDir) {
  if (fs.existsSync(tmpDir)) {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

describe('add_agents duplicate ID handling', function () {
  this.timeout(10000);

  let tmpDir;
  let orchestrator;
  let mockRunner;

  beforeEach(function () {
    tmpDir = createTempDir();
    mockRunner = new MockTaskRunner();
  });

  afterEach(function () {
    if (orchestrator) {
      orchestrator.close();
      orchestrator = null;
    }
    cleanupTempDir(tmpDir);
  });

  it('should REPLACE agent entirely when adding agent with duplicate ID', async function () {
    orchestrator = new Orchestrator({
      dataDir: tmpDir,
      taskRunner: mockRunner,
      quiet: true,
    });

    // Initial agent with QUICK trigger and quick-specific hooks
    const initialConfig = {
      agents: [
        {
          id: 'consensus-coordinator',
          role: 'coordinator',
          modelLevel: 'level2',
          triggers: [{ topic: 'QUICK_VALIDATION_RESULT', action: 'execute_task' }],
          prompt: 'Quick validation coordinator.',
          hooks: {
            onComplete: {
              action: 'publish_message',
              config: { topic: 'QUICK_VALIDATION_PASSED', content: { text: 'Stage 1 passed' } },
            },
          },
        },
      ],
    };

    const result = await orchestrator.start(initialConfig, { text: 'Test task' });
    const clusterId = result.id;
    const cluster = orchestrator.getCluster(clusterId);

    // Verify initial state
    const agentBefore = cluster.agents.find((a) => a.id === 'consensus-coordinator');
    assert.ok(agentBefore, 'consensus-coordinator should exist');
    assert.strictEqual(agentBefore.config.triggers.length, 1);
    assert.strictEqual(agentBefore.config.hooks.onComplete.config.topic, 'QUICK_VALIDATION_PASSED');

    // Add agent with SAME ID but DIFFERENT triggers and hooks (simulating heavy-validation)
    await orchestrator._opAddAgents(
      cluster,
      {
        agents: [
          {
            id: 'consensus-coordinator', // Same ID!
            role: 'coordinator',
            modelLevel: 'level2',
            triggers: [{ topic: 'HEAVY_VALIDATION_RESULT', action: 'execute_task' }],
            prompt: 'Heavy validation coordinator.',
            hooks: {
              onComplete: {
                action: 'publish_message',
                config: { topic: 'VALIDATION_RESULT', content: { text: 'All validations passed' } },
              },
            },
          },
        ],
      },
      {}
    );

    // Verify REPLACEMENT occurred (not merge)
    const clusterAfter = orchestrator.getCluster(clusterId);
    const agentAfter = clusterAfter.agents.find((a) => a.id === 'consensus-coordinator');

    assert.ok(agentAfter, 'consensus-coordinator should still exist');

    // CRITICAL: Should have ONLY the new trigger (not merged)
    assert.strictEqual(
      agentAfter.config.triggers.length,
      1,
      'Should have 1 trigger (replaced, not merged)'
    );
    assert.strictEqual(
      agentAfter.config.triggers[0].topic,
      'HEAVY_VALIDATION_RESULT',
      'Should have the NEW trigger'
    );

    // CRITICAL: Should have NEW hooks (the bug was keeping old hooks)
    assert.strictEqual(
      agentAfter.config.hooks.onComplete.config.topic,
      'VALIDATION_RESULT',
      'Should have NEW hooks (not old QUICK_VALIDATION_PASSED)'
    );

    // Verify prompt was also replaced
    assert.strictEqual(agentAfter.config.prompt, 'Heavy validation coordinator.');

    await orchestrator.stop(clusterId);
  });

  it('should allow adding agents with different IDs', async function () {
    orchestrator = new Orchestrator({
      dataDir: tmpDir,
      taskRunner: mockRunner,
      quiet: true,
    });

    const initialConfig = {
      agents: [
        {
          id: 'quick-consensus',
          role: 'coordinator',
          triggers: [{ topic: 'QUICK_RESULT', action: 'execute_task' }],
          prompt: 'Quick coordinator.',
        },
      ],
    };

    const result = await orchestrator.start(initialConfig, { text: 'Test task' });
    const clusterId = result.id;
    const cluster = orchestrator.getCluster(clusterId);

    // Add a DIFFERENT agent (different ID)
    await orchestrator._opAddAgents(
      cluster,
      {
        agents: [
          {
            id: 'heavy-consensus', // Different ID
            role: 'coordinator',
            triggers: [{ topic: 'HEAVY_RESULT', action: 'execute_task' }],
            prompt: 'Heavy coordinator.',
          },
        ],
      },
      {}
    );

    const clusterAfter = orchestrator.getCluster(clusterId);

    // Should have BOTH agents
    assert.strictEqual(clusterAfter.agents.length, 2, 'Should have 2 agents');

    const quickAgent = clusterAfter.agents.find((a) => a.id === 'quick-consensus');
    const heavyAgent = clusterAfter.agents.find((a) => a.id === 'heavy-consensus');

    assert.ok(quickAgent, 'quick-consensus should exist');
    assert.ok(heavyAgent, 'heavy-consensus should exist');

    await orchestrator.stop(clusterId);
  });

  it('should replace agent instance entirely when same ID added', async function () {
    orchestrator = new Orchestrator({
      dataDir: tmpDir,
      taskRunner: mockRunner,
      quiet: true,
    });

    const initialConfig = {
      agents: [
        {
          id: 'test-agent',
          role: 'validator',
          triggers: [{ topic: 'TEST', action: 'execute_task' }],
          prompt: 'Test agent.',
        },
      ],
    };

    const result = await orchestrator.start(initialConfig, { text: 'Test task' });
    const clusterId = result.id;
    const cluster = orchestrator.getCluster(clusterId);

    // cluster.agents contains AgentWrapper instances directly
    const agentBefore = cluster.agents.find((a) => a.id === 'test-agent');
    assert.ok(agentBefore, 'Agent should exist before replacement');

    // Replace the agent
    await orchestrator._opAddAgents(
      cluster,
      {
        agents: [
          {
            id: 'test-agent',
            role: 'validator',
            triggers: [{ topic: 'TEST2', action: 'execute_task' }],
            prompt: 'Replaced agent.',
          },
        ],
      },
      {}
    );

    const clusterAfter = orchestrator.getCluster(clusterId);
    const agentAfter = clusterAfter.agents.find((a) => a.id === 'test-agent');

    // Should be a DIFFERENT AgentWrapper instance (not the same object)
    assert.notStrictEqual(agentAfter, agentBefore, 'Should be new AgentWrapper instance');

    // Verify the new config was applied
    assert.strictEqual(agentAfter.config.triggers[0].topic, 'TEST2');
    assert.strictEqual(agentAfter.config.prompt, 'Replaced agent.');

    await orchestrator.stop(clusterId);
  });
});
