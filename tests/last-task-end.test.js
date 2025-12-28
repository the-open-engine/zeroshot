/**
 * Test: Verify "since: last_task_end" filters context correctly
 *
 * This test simulates a worker rejecting twice and verifies that:
 * - Iteration 1: No VALIDATION_RESULT messages (first attempt)
 * - Iteration 2: Only sees feedback from iteration 1
 * - Iteration 3: Only sees feedback from iteration 2 (NOT cumulative)
 */

const assert = require('assert');
const AgentWrapper = require('../src/agent-wrapper');
const MessageBus = require('../src/message-bus');
const Ledger = require('../src/ledger');
const path = require('path');
const fs = require('fs');
const os = require('os');

describe('last_task_end - Context Filtering', () => {
  let tempDir;
  let ledger;
  let messageBus;
  let clusterId;
  let clusterCreatedAt;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-test-'));
    const dbPath = path.join(tempDir, 'test-ledger.db');

    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);

    clusterId = 'test-cluster-123';
    clusterCreatedAt = Date.now();
  });

  afterEach(() => {
    if (ledger) ledger.close();
    try {
      fs.rmSync(tempDir, { recursive: true, force: true });
    } catch {
      // Ignore cleanup errors
    }
  });

  it('should filter VALIDATION_RESULT to only show feedback since last task end', async () => {
    // Create worker with "since: last_task_end" for VALIDATION_RESULT
    const workerConfig = {
      id: 'worker',
      role: 'implementation',
      model: 'sonnet',
      timeout: 0,
      contextStrategy: {
        sources: [
          { topic: 'ISSUE_OPENED', limit: 1 },
          { topic: 'VALIDATION_RESULT', since: 'last_task_end', limit: 10 },
        ],
      },
    };

    const mockCluster = {
      id: clusterId,
      createdAt: clusterCreatedAt,
      agents: [],
    };

    const worker = new AgentWrapper(workerConfig, messageBus, mockCluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });

    // Publish initial task
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'system',
      content: { text: 'Implement feature X' },
    });

    // ITERATION 1: Build context (should have NO VALIDATION_RESULT)
    const triggeringMsg1 = {
      topic: 'ISSUE_OPENED',
      sender: 'system',
      timestamp: Date.now(),
    };
    const context1 = worker._buildContext(triggeringMsg1);

    assert.ok(context1.includes('Implement feature X'), 'Should include issue');
    assert.ok(
      !context1.includes('Messages from topic: VALIDATION_RESULT'),
      'Should have no validation results yet'
    );

    // Simulate task 1 completion
    worker.lastTaskEndTime = Date.now();
    await new Promise((resolve) => setTimeout(resolve, 10)); // Ensure timestamp differs

    // Validator rejects iteration 1
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Iteration 1 rejected: Missing error handling',
        data: { approved: false, errors: ['Missing error handling'] },
      },
    });

    await new Promise((resolve) => setTimeout(resolve, 10));

    // ITERATION 2: Build context (should ONLY see iteration 1 feedback)
    const context2 = worker._buildContext(triggeringMsg1);

    assert.ok(
      context2.includes('Messages from topic: VALIDATION_RESULT'),
      'Should include validation results'
    );
    assert.ok(context2.includes('Iteration 1 rejected'), 'Should see iteration 1 feedback');

    // Count how many VALIDATION_RESULT messages
    const validationCount2 = (context2.match(/validator:/g) || []).length;
    assert.strictEqual(validationCount2, 1, 'Should see exactly 1 validation result');

    // Simulate task 2 completion
    worker.lastTaskEndTime = Date.now();
    await new Promise((resolve) => setTimeout(resolve, 10));

    // Validator rejects iteration 2
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: {
        text: 'Iteration 2 rejected: Logic error in validation',
        data: { approved: false, errors: ['Logic error'] },
      },
    });

    await new Promise((resolve) => setTimeout(resolve, 10));

    // ITERATION 3: Build context (should ONLY see iteration 2 feedback, NOT iteration 1)
    const context3 = worker._buildContext(triggeringMsg1);

    assert.ok(
      context3.includes('Messages from topic: VALIDATION_RESULT'),
      'Should include validation results'
    );
    assert.ok(context3.includes('Iteration 2 rejected'), 'Should see iteration 2 feedback');
    assert.ok(
      !context3.includes('Iteration 1 rejected'),
      'Should NOT see iteration 1 feedback (filtered by last_task_end)'
    );

    // Count validation messages - should be exactly 1
    const validationCount3 = (context3.match(/validator:/g) || []).length;
    assert.strictEqual(
      validationCount3,
      1,
      'Should see exactly 1 validation result (not cumulative)'
    );
  });

  it('should fall back to cluster_start if no tasks completed yet', () => {
    const workerConfig = {
      id: 'worker',
      timeout: 0,
      contextStrategy: {
        sources: [{ topic: 'VALIDATION_RESULT', since: 'last_task_end', limit: 10 }],
      },
    };

    const mockCluster = {
      id: clusterId,
      createdAt: clusterCreatedAt,
      agents: [],
    };

    const worker = new AgentWrapper(workerConfig, messageBus, mockCluster, {
      testMode: true,
      mockSpawnFn: () => {},
    });

    // Verify lastTaskEndTime is null initially
    assert.strictEqual(worker.lastTaskEndTime, null, 'Should have no task end time initially');

    // Publish message BEFORE any tasks complete
    messageBus.publish({
      cluster_id: clusterId,
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      content: { text: 'Early validation', data: { approved: false } },
    });

    // Build context - should use cluster.createdAt as fallback
    const triggeringMsg = {
      topic: 'ISSUE_OPENED',
      sender: 'system',
      timestamp: Date.now(),
    };
    const context = worker._buildContext(triggeringMsg);

    // Should see the message (because it was published after cluster start)
    assert.ok(
      context.includes('Early validation'),
      'Should see validation message using cluster_start fallback'
    );
  });
});
