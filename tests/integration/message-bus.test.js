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

describe('MessageBus Integration', function () {
  this.timeout(10000);

  let tempDir;
  let ledger;
  let messageBus;

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
});
