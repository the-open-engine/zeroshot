const assert = require('assert');

const MessageBus = require('../../src/message-bus');
const Ledger = require('../../src/ledger');
const {
  GUIDANCE_BLOCK_START,
  GUIDANCE_BLOCK_END,
  formatGuidanceBlock,
  collectQueuedGuidance,
} = require('../../src/agent/guidance-queue');
const { USER_GUIDANCE_AGENT, USER_GUIDANCE_CLUSTER } = require('../../src/guidance-topics');

describe('Guidance queue formatting', () => {
  it('formats a single delimited guidance block in timestamp order', () => {
    const now = Date.now();
    const messages = [
      {
        topic: USER_GUIDANCE_CLUSTER,
        sender: 'user',
        timestamp: now + 20,
        content: { text: 'Second' },
      },
      {
        topic: USER_GUIDANCE_AGENT,
        sender: 'user',
        timestamp: now + 10,
        content: { text: 'First' },
      },
    ];

    const block = formatGuidanceBlock(messages);

    assert(block.includes('## Guidance (Queued)'), 'block includes header');
    assert(block.includes(GUIDANCE_BLOCK_START), 'block includes start marker');
    assert(block.includes(GUIDANCE_BLOCK_END), 'block includes end marker');

    const firstIndex = block.indexOf('First');
    const secondIndex = block.indexOf('Second');
    assert(firstIndex > -1 && secondIndex > -1, 'block includes both messages');
    assert(firstIndex < secondIndex, 'messages are ordered by timestamp');
  });

  it('returns empty string when no guidance exists', () => {
    assert.strictEqual(formatGuidanceBlock([]), '');
    assert.strictEqual(formatGuidanceBlock(null), '');
  });
});

describe('Guidance queue collection', () => {
  it('deduplicates across iterations via lastDeliveredAt cursor', () => {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const clusterId = 'guidance-queue-1';
    const now = Date.now();

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_CLUSTER,
      sender: 'user',
      content: { text: 'Initial' },
      timestamp: now + 5,
    });

    const first = collectQueuedGuidance({
      messageBus,
      clusterId,
      agentId: 'agent-a',
      lastDeliveredAt: null,
    });

    assert.strictEqual(first.messages.length, 1);
    assert.strictEqual(first.latestTimestamp, now + 5);
    assert(first.guidanceBlock.includes('Initial'));

    const second = collectQueuedGuidance({
      messageBus,
      clusterId,
      agentId: 'agent-a',
      lastDeliveredAt: first.latestTimestamp,
    });

    assert.strictEqual(second.messages.length, 0);
    assert.strictEqual(second.guidanceBlock, '');

    ledger.close();
  });

  it('collects cluster and agent guidance in order', () => {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const clusterId = 'guidance-queue-2';
    const now = Date.now();

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_CLUSTER,
      sender: 'user',
      content: { text: 'Cluster 1' },
      timestamp: now + 10,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_AGENT,
      sender: 'user',
      target_agent_id: 'agent-a',
      content: { text: 'Agent 1' },
      timestamp: now + 20,
    });

    ledger.append({
      cluster_id: clusterId,
      topic: USER_GUIDANCE_CLUSTER,
      sender: 'user',
      content: { text: 'Cluster 2' },
      timestamp: now + 30,
    });

    const result = collectQueuedGuidance({
      messageBus,
      clusterId,
      agentId: 'agent-a',
      lastDeliveredAt: now,
    });

    assert.deepStrictEqual(
      result.messages.map((message) => message.content.text),
      ['Cluster 1', 'Agent 1', 'Cluster 2']
    );

    ledger.close();
  });
});

describe('Guidance queue placement in context', () => {
  it('injects guidance between instructions and JSON output schema', () => {
    const ledger = new Ledger(':memory:');
    const messageBus = new MessageBus(ledger);
    const clusterId = 'guidance-queue-context';
    const cluster = { id: clusterId, createdAt: Date.now() - 1000 };

    const config = {
      id: 'worker',
      role: 'implementation',
      outputFormat: 'json',
      jsonSchema: {
        type: 'object',
        properties: { ok: { type: 'boolean' } },
        required: ['ok'],
      },
      contextStrategy: { sources: [] },
      prompt: 'Do the work.',
    };

    const { buildContext } = require('../../src/agent/agent-context-builder');
    const guidanceBlock = formatGuidanceBlock([
      {
        topic: USER_GUIDANCE_CLUSTER,
        sender: 'user',
        timestamp: Date.now(),
        content: { text: 'Queued guidance' },
      },
    ]);

    const context = buildContext({
      id: 'worker',
      role: 'implementation',
      iteration: 1,
      config,
      messageBus,
      cluster,
      triggeringMessage: {
        cluster_id: clusterId,
        topic: 'ISSUE_OPENED',
        sender: 'tester',
        content: { text: 'Task' },
      },
      queuedGuidance: guidanceBlock,
    });

    const instructionsIndex = context.indexOf('## Instructions');
    const guidanceIndex = context.indexOf('## Guidance (Queued)');
    const outputSchemaIndex = context.indexOf('## 🔴 OUTPUT FORMAT - JSON ONLY');

    assert(instructionsIndex !== -1, 'instructions present');
    assert(guidanceIndex !== -1, 'guidance present');
    assert(outputSchemaIndex !== -1, 'json schema present');
    assert(instructionsIndex < guidanceIndex, 'guidance after instructions');
    assert(guidanceIndex < outputSchemaIndex, 'guidance before json schema');

    ledger.close();
  });
});
