/**
 * Tests for "since: last_agent_start" context filtering.
 *
 * Ensures validators only see IMPLEMENTATION_READY messages published since
 * their previous start time, preventing stale context.
 */

const assert = require('assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const AgentWrapper = require('../src/agent-wrapper');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');

describe('last_agent_start - Context Filtering', () => {
  let tempDir;
  let ledger;
  let messageBus;
  let clusterId;
  let clusterCreatedAt;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-last-agent-start-'));
    const dbPath = path.join(tempDir, 'ledger.db');
    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);
    clusterId = 'cluster-last-agent-start';
    clusterCreatedAt = Date.now();
  });

  afterEach(() => {
    if (ledger) {
      ledger.close();
    }
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      // Ignore cleanup errors
    }
  });

  const createValidator = () => {
    const config = {
      id: 'validator-security',
      role: 'validator',
      timeout: 0,
      contextStrategy: {
        sources: [
          { topic: 'ISSUE_OPENED', limit: 1 },
          { topic: 'PLAN_READY', limit: 1 },
          { topic: 'IMPLEMENTATION_READY', since: 'last_agent_start', limit: 5 },
        ],
      },
    };

    const cluster = {
      id: clusterId,
      createdAt: clusterCreatedAt,
      agents: [],
    };

    return new AgentWrapper(config, messageBus, cluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });
  };

  it('only includes IMPLEMENTATION_READY messages since the previous start', async () => {
    const validator = createValidator();

    // Publish baseline context
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Bug: Fix validation JSON schema' },
    });
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'PLAN_READY',
      sender: 'planner',
      content: { text: 'Plan ready' },
    });

    // Implementation iteration 1 arrives before validator starts
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      content: { text: 'Iteration 1 implementation ready' },
    });

    const trigger = { topic: 'IMPLEMENTATION_READY', sender: 'worker', timestamp: Date.now() };
    const context1 = validator._buildContext(trigger);
    assert.ok(
      context1.includes('Iteration 1 implementation ready'),
      'First run should include iteration 1 implementation'
    );

    // Wait to ensure timestamps differ, publish iteration 2
    await new Promise((resolve) => setTimeout(resolve, 10));
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      content: { text: 'Iteration 2 implementation ready' },
    });

    const context2 = validator._buildContext(trigger);
    assert.ok(
      context2.includes('Iteration 2 implementation ready'),
      'Second run should include new implementation'
    );
    assert.ok(
      !context2.includes('Iteration 1 implementation ready'),
      'Second run should NOT contain iteration 1 implementation'
    );
  });

  it('falls back to cluster start when agent has never run', () => {
    const validator = createValidator();

    messageBus.publish({
      cluster_id: clusterId,
      topic: 'IMPLEMENTATION_READY',
      sender: 'worker',
      content: { text: 'Initial implementation ready' },
    });

    const trigger = { topic: 'IMPLEMENTATION_READY', sender: 'worker', timestamp: Date.now() };
    const context = validator._buildContext(trigger);
    assert.ok(
      context.includes('Initial implementation ready'),
      'First run should include messages published after cluster start'
    );
  });

  it('throws on unknown since tokens to prevent silent context loss', () => {
    const config = {
      id: 'validator-security',
      role: 'validator',
      timeout: 0,
      contextStrategy: {
        sources: [{ topic: 'IMPLEMENTATION_READY', since: 'unknown_token', limit: 1 }],
      },
    };
    const cluster = { id: clusterId, createdAt: clusterCreatedAt, agents: [] };
    const validator = new AgentWrapper(config, messageBus, cluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });

    const trigger = { topic: 'IMPLEMENTATION_READY', sender: 'worker', timestamp: Date.now() };
    assert.throws(
      () => validator._buildContext(trigger),
      /Unknown context source "since" value/,
      'Should surface invalid since values immediately'
    );
  });
});
