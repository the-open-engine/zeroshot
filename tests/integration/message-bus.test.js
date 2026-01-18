/**
 * Integration tests for MessageBus pub/sub layer
 *
 * Tests message validation, topic subscriptions, and event ordering.
 */

const assert = require('node:assert');
const path = require('path');
const fs = require('fs');
const os = require('os');

const MessageBus = require('../../src/message-bus');
const Ledger = require('../../src/ledger');

let tempDir;
let ledger;
let messageBus;

function registerMessageValidationTests() {
  describe('Message Validation', () => {
    it('should require cluster_id', () => {
      assert.throws(() => {
        messageBus.publish({
          topic: 'TEST',
          sender: 'test',
        });
      }, /cluster_id/);
    });

    it('should require topic', () => {
      assert.throws(() => {
        messageBus.publish({
          cluster_id: 'test-cluster',
          sender: 'test',
        });
      }, /topic/);
    });

    it('should require sender', () => {
      assert.throws(() => {
        messageBus.publish({
          cluster_id: 'test-cluster',
          topic: 'TEST',
        });
      }, /sender/);
    });

    it('should accept valid message', () => {
      assert.doesNotThrow(() => {
        messageBus.publish({
          cluster_id: 'test-cluster',
          topic: 'TEST',
          sender: 'test-agent',
          content: { text: 'Hello' },
        });
      });
    });
  });
}

function registerTopicSubscriptionTests() {
  describe('Topic Subscriptions', () => {
    it('should receive messages via subscribe()', (done) => {
      const receivedMessages = [];

      messageBus.subscribe((message) => {
        receivedMessages.push(message);
        if (receivedMessages.length === 2) {
          assert.strictEqual(receivedMessages[0].topic, 'TOPIC_A');
          assert.strictEqual(receivedMessages[1].topic, 'TOPIC_B');
          done();
        }
      });

      messageBus.publish({
        cluster_id: 'test-cluster',
        topic: 'TOPIC_A',
        sender: 'agent-1',
        content: { text: 'First' },
      });

      messageBus.publish({
        cluster_id: 'test-cluster',
        topic: 'TOPIC_B',
        sender: 'agent-2',
        content: { text: 'Second' },
      });
    });

    it('should receive only matching topic via subscribeTopic()', (done) => {
      const receivedMessages = [];

      messageBus.subscribeTopic('TARGET_TOPIC', (message) => {
        receivedMessages.push(message);
      });

      messageBus.publish({
        cluster_id: 'test-cluster',
        topic: 'OTHER_TOPIC',
        sender: 'agent-1',
      });

      messageBus.publish({
        cluster_id: 'test-cluster',
        topic: 'TARGET_TOPIC',
        sender: 'agent-2',
      });

      messageBus.publish({
        cluster_id: 'test-cluster',
        topic: 'ANOTHER_TOPIC',
        sender: 'agent-3',
      });

      setTimeout(() => {
        assert.strictEqual(receivedMessages.length, 1, 'Should only receive TARGET_TOPIC');
        assert.strictEqual(receivedMessages[0].sender, 'agent-2');
        done();
      }, 100);
    });

    it('should support subscribeTopics() for multiple topics', (done) => {
      const receivedMessages = [];

      messageBus.subscribeTopics(['TOPIC_A', 'TOPIC_C'], (message) => {
        receivedMessages.push(message);
      });

      messageBus.publish({ cluster_id: 'c1', topic: 'TOPIC_A', sender: 'a1' });
      messageBus.publish({ cluster_id: 'c1', topic: 'TOPIC_B', sender: 'a2' });
      messageBus.publish({ cluster_id: 'c1', topic: 'TOPIC_C', sender: 'a3' });

      setTimeout(() => {
        assert.strictEqual(receivedMessages.length, 2);
        assert.strictEqual(receivedMessages[0].topic, 'TOPIC_A');
        assert.strictEqual(receivedMessages[1].topic, 'TOPIC_C');
        done();
      }, 100);
    });

    it('should support unsubscribe', (done) => {
      const receivedMessages = [];

      const unsubscribe = messageBus.subscribe((message) => {
        receivedMessages.push(message);
      });

      messageBus.publish({ cluster_id: 'c1', topic: 'T1', sender: 's1' });

      unsubscribe();

      messageBus.publish({ cluster_id: 'c1', topic: 'T2', sender: 's2' });

      setTimeout(() => {
        assert.strictEqual(receivedMessages.length, 1);
        assert.strictEqual(receivedMessages[0].topic, 'T1');
        done();
      }, 100);
    });
  });
}

function registerMessagePersistenceTests() {
  describe('Message Persistence', () => {
    it('should persist messages to ledger', () => {
      messageBus.publish({
        cluster_id: 'persist-test',
        topic: 'PERSISTED',
        sender: 'agent-1',
        content: { text: 'Persisted message' },
      });

      const messages = ledger.query({
        cluster_id: 'persist-test',
        topic: 'PERSISTED',
      });

      assert.strictEqual(messages.length, 1);
      assert.strictEqual(messages[0].content.text, 'Persisted message');
    });

    it('should preserve message ordering', () => {
      const topics = ['A', 'B', 'C', 'D', 'E'];

      topics.forEach((topic, index) => {
        messageBus.publish({
          cluster_id: 'order-test',
          topic,
          sender: `agent-${index}`,
        });
      });

      const messages = ledger.query({ cluster_id: 'order-test' });

      assert.strictEqual(messages.length, 5);
      messages.forEach((msg, index) => {
        assert.strictEqual(
          msg.topic,
          topics[index],
          `Message ${index} should have topic ${topics[index]}`
        );
      });
    });
  });
}

function registerClusterIsolationTests() {
  describe('Cluster Isolation', () => {
    it('should not leak messages between clusters', () => {
      messageBus.publish({
        cluster_id: 'cluster-1',
        topic: 'SHARED_TOPIC',
        sender: 'agent-1',
      });

      messageBus.publish({
        cluster_id: 'cluster-2',
        topic: 'SHARED_TOPIC',
        sender: 'agent-2',
      });

      const cluster1Messages = ledger.query({
        cluster_id: 'cluster-1',
        topic: 'SHARED_TOPIC',
      });

      const cluster2Messages = ledger.query({
        cluster_id: 'cluster-2',
        topic: 'SHARED_TOPIC',
      });

      assert.strictEqual(cluster1Messages.length, 1);
      assert.strictEqual(cluster1Messages[0].sender, 'agent-1');

      assert.strictEqual(cluster2Messages.length, 1);
      assert.strictEqual(cluster2Messages[0].sender, 'agent-2');
    });
  });
}

function registerContentSerializationTests() {
  describe('Content Serialization', () => {
    it('should handle nested content data', () => {
      const complexContent = {
        text: 'Complex message',
        data: {
          nested: {
            array: [1, 2, 3],
            object: { key: 'value' },
          },
          approved: true,
          count: 42,
        },
      };

      messageBus.publish({
        cluster_id: 'serialize-test',
        topic: 'COMPLEX',
        sender: 'agent',
        content: complexContent,
      });

      const messages = ledger.query({
        cluster_id: 'serialize-test',
        topic: 'COMPLEX',
      });

      assert.strictEqual(messages.length, 1);
      assert.deepStrictEqual(messages[0].content.data.nested.array, [1, 2, 3]);
      assert.strictEqual(messages[0].content.data.approved, true);
    });
  });
}

function registerEventEmissionTests() {
  describe('Event Emission', () => {
    it('should emit topic-specific events', (done) => {
      let topicEventReceived = false;

      messageBus.on('topic:SPECIFIC_TOPIC', (message) => {
        topicEventReceived = true;
        assert.strictEqual(message.topic, 'SPECIFIC_TOPIC');
      });

      messageBus.publish({
        cluster_id: 'event-test',
        topic: 'SPECIFIC_TOPIC',
        sender: 'agent',
      });

      setTimeout(() => {
        assert(topicEventReceived, 'Should have received topic-specific event');
        done();
      }, 100);
    });

    it('should emit generic message event', (done) => {
      let genericEventReceived = false;

      messageBus.on('message', (message) => {
        genericEventReceived = true;
        assert.strictEqual(message.topic, 'ANY_TOPIC');
      });

      messageBus.publish({
        cluster_id: 'event-test',
        topic: 'ANY_TOPIC',
        sender: 'agent',
      });

      setTimeout(() => {
        assert(genericEventReceived, 'Should have received generic message event');
        done();
      }, 100);
    });
  });
}

function registerBatchPublishingTests() {
  describe('Batch Publishing (Atomic)', () => {
    registerBatchPublishingBasics();
    registerBatchPublishingTimestamps();
    registerBatchPublishingEvents();
    registerBatchPublishingValidation();
    registerBatchPublishingOrdering();
    registerBatchPublishingInterleaving();
  });
}

function registerBatchPublishingBasics() {
  it('should publish multiple messages atomically', () => {
    const messages = [
      {
        cluster_id: 'batch-test',
        topic: 'TOKEN_USAGE',
        sender: 'worker',
        content: { data: { tokens: 100 } },
      },
      {
        cluster_id: 'batch-test',
        topic: 'TASK_COMPLETED',
        sender: 'worker',
        content: { text: 'Done' },
      },
      {
        cluster_id: 'batch-test',
        topic: 'HOOK_RESULT',
        sender: 'worker',
        content: { data: { approved: true } },
      },
    ];

    const published = messageBus.batchPublish(messages);

    assert.strictEqual(published.length, 3);
    assert.strictEqual(published[0].topic, 'TOKEN_USAGE');
    assert.strictEqual(published[1].topic, 'TASK_COMPLETED');
    assert.strictEqual(published[2].topic, 'HOOK_RESULT');
  });
}

function registerBatchPublishingTimestamps() {
  it('should assign contiguous timestamps to batch messages', () => {
    const messages = [
      { cluster_id: 'batch-test', topic: 'MSG_1', sender: 'agent' },
      { cluster_id: 'batch-test', topic: 'MSG_2', sender: 'agent' },
      { cluster_id: 'batch-test', topic: 'MSG_3', sender: 'agent' },
    ];

    const published = messageBus.batchPublish(messages);

    for (let i = 1; i < published.length; i++) {
      assert.strictEqual(
        published[i].timestamp,
        published[i - 1].timestamp + 1,
        `Message ${i} should have timestamp exactly 1ms after message ${i - 1}`
      );
    }
  });
}

function registerBatchPublishingEvents() {
  it('should emit topic events for all batch messages', (done) => {
    const receivedTopics = [];

    messageBus.subscribeTopic('BATCH_TOPIC_A', (msg) => receivedTopics.push(msg.topic));
    messageBus.subscribeTopic('BATCH_TOPIC_B', (msg) => receivedTopics.push(msg.topic));

    messageBus.batchPublish([
      { cluster_id: 'batch-test', topic: 'BATCH_TOPIC_A', sender: 'agent' },
      { cluster_id: 'batch-test', topic: 'BATCH_TOPIC_B', sender: 'agent' },
    ]);

    setTimeout(() => {
      assert.strictEqual(receivedTopics.length, 2);
      assert(receivedTopics.includes('BATCH_TOPIC_A'));
      assert(receivedTopics.includes('BATCH_TOPIC_B'));
      done();
    }, 100);
  });
}

function registerBatchPublishingValidation() {
  it('should validate all messages before publishing any', () => {
    const messages = [
      { cluster_id: 'batch-test', topic: 'VALID', sender: 'agent' },
      { cluster_id: 'batch-test', topic: 'MISSING_SENDER' }, // Missing sender
    ];

    assert.throws(() => {
      messageBus.batchPublish(messages);
    }, /sender.*required/i);

    const allMessages = ledger.query({ cluster_id: 'batch-test' });
    assert.strictEqual(allMessages.length, 0);
  });

  it('should return empty array for empty batch', () => {
    const published = messageBus.batchPublish([]);
    assert.deepStrictEqual(published, []);
  });
}

function registerBatchPublishingOrdering() {
  it('should preserve message ordering in batch', () => {
    const messages = [];
    for (let i = 0; i < 10; i++) {
      messages.push({
        cluster_id: 'order-batch-test',
        topic: `TOPIC_${i}`,
        sender: 'agent',
      });
    }

    messageBus.batchPublish(messages);

    const stored = ledger.query({ cluster_id: 'order-batch-test' });
    assert.strictEqual(stored.length, 10);

    for (let i = 0; i < 10; i++) {
      assert.strictEqual(stored[i].topic, `TOPIC_${i}`);
    }
  });
}

function registerBatchPublishingInterleaving() {
  it('should prevent interleaving with concurrent agents', () => {
    const agentAMessages = [
      { cluster_id: 'interleave-test', topic: 'A_TOKEN_USAGE', sender: 'agent-A' },
      { cluster_id: 'interleave-test', topic: 'A_COMPLETED', sender: 'agent-A' },
    ];

    const agentBMessages = [
      { cluster_id: 'interleave-test', topic: 'B_TOKEN_USAGE', sender: 'agent-B' },
      { cluster_id: 'interleave-test', topic: 'B_COMPLETED', sender: 'agent-B' },
    ];

    messageBus.batchPublish(agentAMessages);
    messageBus.batchPublish(agentBMessages);

    const stored = ledger.query({ cluster_id: 'interleave-test' });

    const agentAIndices = stored
      .map((m, idx) => (m.sender === 'agent-A' ? idx : -1))
      .filter((idx) => idx >= 0);
    assert.strictEqual(
      agentAIndices[1] - agentAIndices[0],
      1,
      "Agent A's messages should be contiguous"
    );

    const agentBIndices = stored
      .map((m, idx) => (m.sender === 'agent-B' ? idx : -1))
      .filter((idx) => idx >= 0);
    assert.strictEqual(
      agentBIndices[1] - agentBIndices[0],
      1,
      "Agent B's messages should be contiguous"
    );
  });
}

function registerTaskIdCausalLinkingTests() {
  describe('TaskId Causal Linking (Multi-Agent)', () => {
    registerTaskIdGroupingTest();
    registerTaskIdOrderingTest();
    registerTaskIdTokenTotalsTest();
  });
}

function registerTaskIdGroupingTest() {
  it('should allow grouping messages by taskId even when interleaved', () => {
    const taskIdA = 'worker-1735398000000-1';
    const taskIdB = 'worker-1735398000100-2';

    messageBus.publish({
      cluster_id: 'causal-test',
      topic: 'TOKEN_USAGE',
      sender: 'agent-A',
      content: { data: { taskId: taskIdA, tokens: 100 } },
    });

    messageBus.publish({
      cluster_id: 'causal-test',
      topic: 'TOKEN_USAGE',
      sender: 'agent-B',
      content: { data: { taskId: taskIdB, tokens: 200 } },
    });

    messageBus.publish({
      cluster_id: 'causal-test',
      topic: 'TASK_COMPLETED',
      sender: 'agent-A',
      content: { data: { taskId: taskIdA, success: true } },
    });

    messageBus.publish({
      cluster_id: 'causal-test',
      topic: 'VALIDATION_RESULT',
      sender: 'agent-A',
      content: { data: { taskId: taskIdA, approved: true } },
    });

    messageBus.publish({
      cluster_id: 'causal-test',
      topic: 'TASK_COMPLETED',
      sender: 'agent-B',
      content: { data: { taskId: taskIdB, success: true } },
    });

    const stored = ledger.query({ cluster_id: 'causal-test' });

    const groupedByTask = {};
    for (const msg of stored) {
      const taskId = msg.content?.data?.taskId;
      if (taskId) {
        if (!groupedByTask[taskId]) {
          groupedByTask[taskId] = [];
        }
        groupedByTask[taskId].push(msg);
      }
    }

    assert.strictEqual(groupedByTask[taskIdA].length, 3, 'Task A should have 3 messages');
    assert.strictEqual(groupedByTask[taskIdA][0].topic, 'TOKEN_USAGE');
    assert.strictEqual(groupedByTask[taskIdA][1].topic, 'TASK_COMPLETED');
    assert.strictEqual(groupedByTask[taskIdA][2].topic, 'VALIDATION_RESULT');

    assert.strictEqual(groupedByTask[taskIdB].length, 2, 'Task B should have 2 messages');
    assert.strictEqual(groupedByTask[taskIdB][0].topic, 'TOKEN_USAGE');
    assert.strictEqual(groupedByTask[taskIdB][1].topic, 'TASK_COMPLETED');
  });
}

function registerTaskIdOrderingTest() {
  it('should maintain correct order within each task group by timestamp', () => {
    const taskId = 'worker-order-test-1';

    const baseTime = Date.now();

    messageBus.publish({
      cluster_id: 'order-causal-test',
      topic: 'TOKEN_USAGE',
      sender: 'worker',
      timestamp: baseTime + 0,
      content: { data: { taskId, phase: 'start' } },
    });

    messageBus.publish({
      cluster_id: 'order-causal-test',
      topic: 'VALIDATION_RESULT',
      sender: 'validator',
      timestamp: baseTime + 100,
      content: { data: { taskId, phase: 'validate' } },
    });

    messageBus.publish({
      cluster_id: 'order-causal-test',
      topic: 'TASK_COMPLETED',
      sender: 'worker',
      timestamp: baseTime + 200,
      content: { data: { taskId, phase: 'complete' } },
    });

    const stored = ledger.query({ cluster_id: 'order-causal-test' });

    const taskMessages = stored.filter((m) => m.content?.data?.taskId === taskId);

    assert.strictEqual(taskMessages.length, 3);
    assert.strictEqual(taskMessages[0].content.data.phase, 'start');
    assert.strictEqual(taskMessages[1].content.data.phase, 'validate');
    assert.strictEqual(taskMessages[2].content.data.phase, 'complete');

    for (let i = 1; i < taskMessages.length; i++) {
      assert(
        taskMessages[i].timestamp >= taskMessages[i - 1].timestamp,
        `Message ${i} should have timestamp >= message ${i - 1}`
      );
    }
  });
}

function registerTaskIdTokenTotalsTest() {
  it('should calculate correct token totals per task via taskId grouping', () => {
    const taskId1 = 'worker-iter-1';
    const taskId2 = 'worker-iter-2';

    messageBus.publish({
      cluster_id: 'token-test',
      topic: 'TOKEN_USAGE',
      sender: 'worker',
      content: { data: { taskId: taskId1, inputTokens: 1000, outputTokens: 500 } },
    });

    messageBus.publish({
      cluster_id: 'token-test',
      topic: 'TOKEN_USAGE',
      sender: 'worker',
      content: { data: { taskId: taskId2, inputTokens: 2000, outputTokens: 800 } },
    });

    messageBus.publish({
      cluster_id: 'token-test',
      topic: 'TOKEN_USAGE',
      sender: 'validator',
      content: { data: { taskId: 'validator-task-1', inputTokens: 500, outputTokens: 200 } },
    });

    const stored = ledger.query({ cluster_id: 'token-test', topic: 'TOKEN_USAGE' });

    const tokensByTask = {};
    for (const msg of stored) {
      const taskId = msg.content?.data?.taskId;
      if (!tokensByTask[taskId]) {
        tokensByTask[taskId] = { input: 0, output: 0 };
      }
      tokensByTask[taskId].input += msg.content?.data?.inputTokens || 0;
      tokensByTask[taskId].output += msg.content?.data?.outputTokens || 0;
    }

    assert.strictEqual(tokensByTask[taskId1].input, 1000);
    assert.strictEqual(tokensByTask[taskId1].output, 500);
    assert.strictEqual(tokensByTask[taskId2].input, 2000);
    assert.strictEqual(tokensByTask[taskId2].output, 800);
    assert.strictEqual(tokensByTask['validator-task-1'].input, 500);
    assert.strictEqual(tokensByTask['validator-task-1'].output, 200);
  });
}

describe('MessageBus Integration', function () {
  this.timeout(10000);

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'zeroshot-msgbus-test-'));
    const dbPath = path.join(tempDir, 'test.db');
    ledger = new Ledger(dbPath);
    messageBus = new MessageBus(ledger);
  });

  afterEach(() => {
    if (ledger) ledger.close();
    if (tempDir && fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true, force: true });
    }
  });

  registerMessageValidationTests();
  registerTopicSubscriptionTests();
  registerMessagePersistenceTests();
  registerClusterIsolationTests();
  registerContentSerializationTests();
  registerEventEmissionTests();
  registerBatchPublishingTests();
  registerTaskIdCausalLinkingTests();
});
