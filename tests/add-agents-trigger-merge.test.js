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


  it('should allow adding agents with different IDs', async function () {
    orchestrator = new Orchestrator({
      storageDir: tmpDir,
      skipLoad: true,
      taskRunner: mockRunner,
      quiet: true,
    });

    // Configure mock behaviors
    mockRunner.when('quick-consensus').returns(JSON.stringify({ result: 'ok' }));
    mockRunner.when('heavy-consensus').returns(JSON.stringify({ result: 'ok' }));

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
      storageDir: tmpDir,
      skipLoad: true,
      taskRunner: mockRunner,
      quiet: true,
    });

    // Configure mock behavior for test-agent
    mockRunner.when('test-agent').returns({ result: 'ok' });

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
