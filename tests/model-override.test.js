const assert = require('assert');
const fs = require('fs');
const path = require('path');
const os = require('os');
const Orchestrator = require('../src/orchestrator');
const MockTaskRunner = require('./helpers/mock-task-runner');

describe('Model Override (--model flag)', () => {
  let orchestrator;
  let storageDir;
  let settingsFile;

  beforeEach(function () {
    // Create temporary storage directory for each test
    storageDir = path.join(os.tmpdir(), `zeroshot-test-${Date.now()}-${Math.random()}`);
    fs.mkdirSync(storageDir, { recursive: true });

    settingsFile = path.join(storageDir, 'settings.json');
    process.env.ZEROSHOT_SETTINGS_FILE = settingsFile;
    fs.writeFileSync(settingsFile, JSON.stringify({ maxModel: 'opus' }, null, 2), 'utf8');

    orchestrator = new Orchestrator({
      storageDir,
      quiet: true,
      skipLoad: true,
      taskRunner: new MockTaskRunner(),
    });
  });

  afterEach(function () {
    orchestrator.close();
    try {
      fs.rmSync(storageDir, { recursive: true, force: true });
    } catch (e) {
      console.error('Cleanup failed:', e.message);
    }
    delete process.env.ZEROSHOT_SETTINGS_FILE;
  });

  it('should override all agent models when modelOverride is provided', async function () {
    const config = {
      agents: [
        { id: 'agent1', model: 'haiku', role: 'worker', triggers: [] },
        {
          id: 'agent2',
          modelRules: [{ iterations: 'all', model: 'sonnet' }],
          role: 'validator',
          triggers: [],
        },
        { id: 'agent3', role: 'planner', triggers: [] }, // No model specified
      ],
    };

    const result = await orchestrator.start(
      config,
      { text: 'test task' },
      { modelOverride: 'opus' }
    );

    // Get the actual cluster object from orchestrator
    const cluster = orchestrator.clusters.get(result.id);

    // Verify cluster stores the model override
    assert.strictEqual(cluster.modelOverride, 'opus');

    // Verify all agents use the overridden model
    assert.strictEqual(cluster.agents.length, 3);
    for (const agent of cluster.agents) {
      // Use _selectModel() to get the actual model the agent will use
      assert.strictEqual(
        agent._selectModel(),
        'opus',
        `Agent ${agent.id} should use model 'opus' but uses '${agent._selectModel()}'`
      );
    }
  });

  it('should persist modelOverride for dynamically added agents', async function () {
    const config = {
      agents: [
        {
          id: 'conductor',
          model: 'sonnet',
          role: 'conductor',
          triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
        },
      ],
    };

    const result = await orchestrator.start(
      config,
      { text: 'test task' },
      { modelOverride: 'opus', testMode: true }
    );

    // Get the actual cluster object from orchestrator
    const cluster = orchestrator.clusters.get(result.id);

    // Verify modelOverride is stored in cluster
    assert.strictEqual(cluster.modelOverride, 'opus');

    // Simulate conductor adding a new agent via CLUSTER_OPERATIONS
    const newAgentConfig = {
      id: 'worker',
      modelRules: [{ iterations: 'all', model: 'haiku' }], // This should be overridden
      role: 'implementation',
      triggers: [],
    };

    // Call _opAddAgents directly to test the override logic
    await orchestrator._opAddAgents(cluster, { agents: [newAgentConfig] }, {});

    // Verify the dynamically added agent has the overridden model
    const addedAgent = cluster.agents.find((a) => a.id === 'worker');
    assert.ok(addedAgent, 'Worker agent should be added');
    assert.strictEqual(
      addedAgent._selectModel(),
      'opus',
      'Dynamically added agent should have overridden model'
    );
  });

  it('should not override models when modelOverride is not provided', async function () {
    const config = {
      agents: [
        { id: 'agent1', model: 'haiku', role: 'worker', triggers: [] },
        { id: 'agent2', model: 'sonnet', role: 'validator', triggers: [] },
      ],
    };

    const result = await orchestrator.start(config, { text: 'test task' });

    // Get the actual cluster object from orchestrator
    const cluster = orchestrator.clusters.get(result.id);

    // Verify agents keep their original models
    const agent1 = cluster.agents.find((a) => a.id === 'agent1');
    const agent2 = cluster.agents.find((a) => a.id === 'agent2');

    assert.strictEqual(agent1._selectModel(), 'haiku');
    assert.strictEqual(agent2._selectModel(), 'sonnet');
  });

  it('should preserve modelOverride when cluster is saved and loaded', async function () {
    const config = {
      agents: [{ id: 'agent1', model: 'sonnet', role: 'worker', triggers: [] }],
    };

    const cluster = await orchestrator.start(
      config,
      { text: 'test task' },
      { modelOverride: 'opus' }
    );

    // Save clusters
    await orchestrator._saveClusters();

    // Create new orchestrator and load clusters
    const newOrchestrator = await Orchestrator.create({
      storageDir,
      quiet: true,
      taskRunner: new MockTaskRunner(),
    });

    const loadedCluster = newOrchestrator.clusters.get(cluster.id);
    assert.ok(loadedCluster, 'Cluster should be loaded');
    assert.strictEqual(loadedCluster.modelOverride, 'opus', 'Model override should be persisted');

    newOrchestrator.close();
  });
});
