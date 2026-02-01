/**
 * Regression test: add_agents should merge triggers for duplicate agent IDs
 *
 * BUG: When heavy-validation template adds consensus-coordinator (which already
 * exists from quick-validation), the HEAVY_VALIDATION_RESULT trigger was lost
 * because add_agents skipped duplicate agent IDs entirely.
 *
 * FIX: Merge triggers from new config into existing agent instead of skipping.
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
  return fs.mkdtempSync(path.join(tmpBase, 'trigger-merge-'));
}

function cleanupTempDir(tmpDir) {
  if (fs.existsSync(tmpDir)) {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

describe('add_agents trigger merge', function () {
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

  it('should merge triggers when adding agent with duplicate ID', async function () {
    // Create orchestrator with mock task runner
    orchestrator = new Orchestrator({
      dataDir: tmpDir,
      taskRunner: mockRunner,
      quiet: true,
    });

    // Create initial config with consensus-coordinator that has QUICK trigger
    const initialConfig = {
      agents: [
        {
          id: 'consensus-coordinator',
          role: 'coordinator',
          modelLevel: 'level2',
          triggers: [
            {
              topic: 'QUICK_VALIDATION_RESULT',
              action: 'execute_task',
            },
          ],
          prompt: 'You are the consensus coordinator.',
        },
      ],
    };

    // Start cluster
    const result = await orchestrator.start(initialConfig, { text: 'Test task' });
    const clusterId = result.id;

    // Get cluster to access agents directly (getStatus doesn't include triggers)
    const cluster = orchestrator.getCluster(clusterId);

    // Verify initial state - only QUICK trigger (triggers are in agent.config.triggers)
    const agentBefore = cluster.agents.find((a) => a.id === 'consensus-coordinator');
    assert.ok(agentBefore, 'consensus-coordinator should exist');
    assert.strictEqual(
      agentBefore.config.triggers?.length || 0,
      1,
      'Should have 1 trigger initially'
    );
    await orchestrator._opAddAgents(
      cluster,
      {
        agents: [
          {
            id: 'consensus-coordinator', // Same ID!
            role: 'coordinator',
            modelLevel: 'level2',
            triggers: [
              {
                topic: 'HEAVY_VALIDATION_RESULT', // Different trigger
                action: 'execute_task',
              },
            ],
            prompt: 'You are the consensus coordinator for heavy validation.',
          },
        ],
      },
      {}
    );

    // Verify triggers were MERGED, not skipped
    // Re-fetch cluster to get updated agent state
    const clusterAfter = orchestrator.getCluster(clusterId);
    const agentAfter = clusterAfter.agents.find((a) => a.id === 'consensus-coordinator');

    assert.ok(agentAfter, 'consensus-coordinator should still exist');

    // THE CRITICAL ASSERTION: Should now have BOTH triggers (triggers in agent.config.triggers)
    const triggers = agentAfter.config.triggers || [];
    assert.strictEqual(triggers.length, 2, 'Should have 2 triggers after merge');

    const hasQuickTrigger = triggers.some((t) => t.topic === 'QUICK_VALIDATION_RESULT');
    const hasHeavyTrigger = triggers.some((t) => t.topic === 'HEAVY_VALIDATION_RESULT');

    assert.ok(hasQuickTrigger, 'Should still have QUICK_VALIDATION_RESULT trigger');
    assert.ok(hasHeavyTrigger, 'Should have merged HEAVY_VALIDATION_RESULT trigger');

    await orchestrator.stop(clusterId);
  });

  it('should not duplicate identical triggers', async function () {
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
          modelLevel: 'level2',
          triggers: [
            {
              topic: 'TEST_TOPIC',
              action: 'execute_task',
            },
          ],
          prompt: 'Test agent.',
        },
      ],
    };

    const result = await orchestrator.start(initialConfig, { text: 'Test task' });
    const clusterId = result.id;

    const cluster = orchestrator.getCluster(clusterId);

    // Try to add agent with SAME trigger (should not duplicate)
    await orchestrator._opAddAgents(
      cluster,
      {
        agents: [
          {
            id: 'test-agent',
            role: 'validator',
            triggers: [
              {
                topic: 'TEST_TOPIC', // Same trigger
                action: 'execute_task',
              },
            ],
            prompt: 'Test agent duplicate.',
          },
        ],
      },
      {}
    );

    // Re-fetch cluster to get updated agent state (triggers in agent.config.triggers)
    const clusterAfter = orchestrator.getCluster(clusterId);
    const agent = clusterAfter.agents.find((a) => a.id === 'test-agent');
    const triggers = agent.config.triggers || [];

    // Should still have only 1 trigger (no duplication)
    assert.strictEqual(triggers.length, 1, 'Should not duplicate identical triggers');

    await orchestrator.stop(clusterId);
  });
});
