const assert = require('assert');

const AgentWrapper = require('../../src/agent-wrapper');
const MessageBus = require('../../src/message-bus');
const Ledger = require('../../src/ledger');
const MockTaskRunner = require('../helpers/mock-task-runner');
const { USER_GUIDANCE_AGENT } = require('../../src/guidance-topics');

describe('Guidance queue integration', function () {
  it('injects queued guidance into next prompt only', async function () {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const mockRunner = new MockTaskRunner();
    const clusterId = 'guidance-queue-integration';
    const clusterCreatedAt = Date.now() - 5000;

    const cluster = {
      id: clusterId,
      createdAt: clusterCreatedAt,
      agents: [],
    };

    const workerConfig = {
      id: 'worker',
      role: 'implementation',
      modelLevel: 'level2',
      timeout: 0,
      maxIterations: 5,
      triggers: [{ topic: 'ISSUE_OPENED', action: 'execute_task' }],
      contextStrategy: {
        sources: [{ topic: 'ISSUE_OPENED', since: 'cluster_start', limit: 1 }],
      },
    };

    const worker = new AgentWrapper(workerConfig, messageBus, cluster, {
      testMode: true,
      taskRunner: mockRunner,
    });
    cluster.agents.push(worker);

    const trigger = {
      cluster_id: clusterId,
      topic: 'ISSUE_OPENED',
      sender: 'tester',
      content: { text: 'Implement feature X' },
    };

    mockRunner.when('worker').returns({ ok: true });

    worker.start();

    // First run: no guidance queued
    await worker._executeTask(trigger);
    mockRunner.assertCalled('worker', 1);
    const firstContext = mockRunner.calls[0].context;
    assert(!firstContext.includes('## Guidance (Queued)'), 'no guidance block in first run');

    // Queue guidance after first execution
    messageBus.publish({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'worker',
      content: { text: 'Use approach B' },
      timestamp: Date.now() + 10,
    });

    await worker._executeTask(trigger);
    mockRunner.assertCalled('worker', 2);
    const secondContext = mockRunner.calls[1].context;
    assert(secondContext.includes('## Guidance (Queued)'), 'guidance block appears on next run');
    assert(secondContext.includes('Use approach B'), 'guidance text is included');

    // Third run: no new guidance
    await worker._executeTask(trigger);
    mockRunner.assertCalled('worker', 3);
    const thirdContext = mockRunner.calls[2].context;
    assert(!thirdContext.includes('## Guidance (Queued)'), 'guidance block not repeated');

    await worker.stop();
    ledger.close();
  });
});
