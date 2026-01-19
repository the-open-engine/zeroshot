/**
 * Tests for LedgerAssertions helper
 */

const assert = require('assert');
const Ledger = require('../../src/ledger');
const LedgerAssertions = require('./ledger-assertions');

let ledger;
let assertions;
const clusterId = 'test-cluster-123';

describe('LedgerAssertions', function () {
  beforeEach(() => {
    ledger = new Ledger(':memory:');
    assertions = new LedgerAssertions(ledger, clusterId);
  });

  registerConstructorTests();
  registerAssertPublishedTests();
  registerAssertCountTests();
  registerAssertSequenceTests();
  registerLastMessageTests();
  registerGetMessagesTests();
  registerMethodChainingTests();
  registerIsolationTests();
});

function registerConstructorTests() {
  describe('constructor', function () {
    it('should require ledger', function () {
      assert.throws(() => new LedgerAssertions(null, 'cluster-1'), /ledger is required/);
    });

    it('should require clusterId', function () {
      assert.throws(() => new LedgerAssertions(ledger, null), /clusterId is required/);
    });

    it('should accept valid ledger and clusterId', function () {
      const a = new LedgerAssertions(ledger, 'cluster-1');
      assert.ok(a);
    });
  });
}

function registerAssertPublishedTests() {
  describe('assertPublished', function () {
    it('should pass when topic exists', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'ISSUE_OPENED',
        sender: 'test-agent',
        receiver: 'broadcast',
        content: { text: 'Test message' },
      });

      // Should not throw
      assertions.assertPublished('ISSUE_OPENED');
    });

    it('should fail when topic does not exist', function () {
      assert.throws(
        () => assertions.assertPublished('NONEXISTENT_TOPIC'),
        /Expected at least one message on topic "NONEXISTENT_TOPIC"/
      );
    });

    it('should filter by sender', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'RESULT',
        sender: 'agent-a',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'RESULT',
        sender: 'agent-b',
        receiver: 'broadcast',
      });

      // Should pass with correct sender
      assertions.assertPublished('RESULT', { sender: 'agent-a' });
      assertions.assertPublished('RESULT', { sender: 'agent-b' });
    });

    it('should fail if sender filter does not match', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'RESULT',
        sender: 'agent-a',
        receiver: 'broadcast',
      });

      assert.throws(
        () => assertions.assertPublished('RESULT', { sender: 'agent-x' }),
        /Expected at least one message on topic "RESULT"/
      );
    });

    it('should support method chaining', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TOPIC1',
        sender: 'agent-a',
        receiver: 'broadcast',
      });

      const result = assertions.assertPublished('TOPIC1');
      assert.strictEqual(result, assertions, 'Should return this for chaining');
    });
  });
}

function registerAssertCountTests() {
  describe('assertCount', function () {
    it('should pass when count matches', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-a',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-b',
        receiver: 'broadcast',
      });

      // Should not throw
      assertions.assertCount('TEST', 2);
    });

    it('should fail when count does not match', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-a',
        receiver: 'broadcast',
      });

      assert.throws(
        () => assertions.assertCount('TEST', 5),
        /Expected 5 messages on topic "TEST", but found 1/
      );
    });

    it('should pass for zero count on nonexistent topic', function () {
      assertions.assertCount('NONEXISTENT', 0);
    });

    it('should fail for nonzero count on nonexistent topic', function () {
      assert.throws(
        () => assertions.assertCount('NONEXISTENT', 1),
        /Expected 1 messages on topic "NONEXISTENT", but found 0/
      );
    });

    it('should support method chaining', function () {
      const result = assertions.assertCount('ANY', 0);
      assert.strictEqual(result, assertions, 'Should return this for chaining');
    });
  });
}

function registerAssertSequenceTests() {
  describe('assertSequence', function () {
    it('should pass when topics appear in order', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'ISSUE_OPENED',
        sender: 'planner',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'PLAN_READY',
        sender: 'planner',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'IMPLEMENTATION_READY',
        sender: 'worker',
        receiver: 'broadcast',
      });

      // Should not throw
      assertions.assertSequence(['ISSUE_OPENED', 'PLAN_READY', 'IMPLEMENTATION_READY']);
    });

    it('should allow non-consecutive topics', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TOPIC_A',
        sender: 'agent',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TOPIC_X', // Extra topic in between
        sender: 'agent',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TOPIC_B',
        sender: 'agent',
        receiver: 'broadcast',
      });

      // Should pass - doesn't require consecutive topics
      assertions.assertSequence(['TOPIC_A', 'TOPIC_B']);
    });

    it('should fail when topics are in wrong order', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'IMPLEMENTATION_READY',
        sender: 'worker',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'PLAN_READY',
        sender: 'planner',
        receiver: 'broadcast',
      });

      assert.throws(
        () => assertions.assertSequence(['PLAN_READY', 'IMPLEMENTATION_READY']),
        /Missing topic "IMPLEMENTATION_READY"/
      );
    });

    it('should fail when topic is missing', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TOPIC_A',
        sender: 'agent',
        receiver: 'broadcast',
      });

      // Missing TOPIC_B
      assert.throws(
        () => assertions.assertSequence(['TOPIC_A', 'TOPIC_B']),
        /Missing topic "TOPIC_B"/
      );
    });

    it('should provide helpful error with actual sequence', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'WRONG_A',
        sender: 'agent',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'WRONG_B',
        sender: 'agent',
        receiver: 'broadcast',
      });

      assert.throws(
        () => assertions.assertSequence(['EXPECTED_A', 'EXPECTED_B']),
        /Actual sequence.*WRONG_A → WRONG_B/
      );
    });

    it('should handle empty ledger', function () {
      assert.throws(() => assertions.assertSequence(['TOPIC_A']), /Missing topic "TOPIC_A"/);
    });

    it('should support method chaining', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'A',
        sender: 'agent',
        receiver: 'broadcast',
      });

      const result = assertions.assertSequence(['A']);
      assert.strictEqual(result, assertions, 'Should return this for chaining');
    });
  });
}

function registerLastMessageTests() {
  describe('lastMessage', function () {
    it('should return null when topic does not exist', function () {
      const result = assertions.lastMessage('NONEXISTENT');
      assert.strictEqual(result, null);
    });

    it('should return the last message for a topic', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-a',
        receiver: 'broadcast',
        content: { text: 'First' },
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-b',
        receiver: 'broadcast',
        content: { text: 'Second' },
      });

      const result = assertions.lastMessage('TEST');
      assert.ok(result);
      assert.strictEqual(result.sender, 'agent-b');
      assert.strictEqual(result.content.text, 'Second');
    });

    it('should ignore messages from other clusters', function () {
      ledger.append({
        cluster_id: 'other-cluster',
        topic: 'TEST',
        sender: 'other-agent',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'target-agent',
        receiver: 'broadcast',
      });

      const result = assertions.lastMessage('TEST');
      assert.strictEqual(result.sender, 'target-agent');
    });
  });
}

function registerGetMessagesTests() {
  describe('getMessages', function () {
    it('should return empty array when topic does not exist', function () {
      const result = assertions.getMessages('NONEXISTENT');
      assert.deepStrictEqual(result, []);
    });

    it('should return all messages for a topic', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-a',
        receiver: 'broadcast',
        content: { text: 'First' },
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'agent-b',
        receiver: 'broadcast',
        content: { text: 'Second' },
      });

      const result = assertions.getMessages('TEST');
      assert.strictEqual(result.length, 2);
      assert.strictEqual(result[0].sender, 'agent-a');
      assert.strictEqual(result[1].sender, 'agent-b');
    });

    it('should ignore messages from other clusters', function () {
      ledger.append({
        cluster_id: 'other-cluster',
        topic: 'TEST',
        sender: 'other-agent',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'target-agent-1',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'TEST',
        sender: 'target-agent-2',
        receiver: 'broadcast',
      });

      const result = assertions.getMessages('TEST');
      assert.strictEqual(result.length, 2);
      assert.strictEqual(result[0].sender, 'target-agent-1');
      assert.strictEqual(result[1].sender, 'target-agent-2');
    });

    it('should return messages in timestamp order', function () {
      const baseTime = Date.now();

      // Bypass Ledger.append monotonic timestamp enforcement so we can verify ordering
      // against out-of-order timestamps that may exist in persisted ledgers.
      const insert = ledger.db.prepare(
        'INSERT INTO messages (id, timestamp, topic, sender, receiver, content_text, content_data, metadata, cluster_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)'
      );
      insert.run(
        'msg_a',
        baseTime + 1000,
        'TEST',
        'agent-a',
        'broadcast',
        null,
        null,
        null,
        clusterId
      );
      insert.run('msg_b', baseTime, 'TEST', 'agent-b', 'broadcast', null, null, null, clusterId);
      ledger.cache.clear();

      const result = assertions.getMessages('TEST');
      assert.strictEqual(result.length, 2);
      assert.strictEqual(result[0].timestamp, baseTime);
      assert.strictEqual(result[1].timestamp, baseTime + 1000);
    });
  });
}

function registerMethodChainingTests() {
  describe('method chaining', function () {
    it('should allow chaining multiple assertions', function () {
      ledger.append({
        cluster_id: clusterId,
        topic: 'ISSUE_OPENED',
        sender: 'user',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: clusterId,
        topic: 'ANALYSIS_COMPLETE',
        sender: 'planner',
        receiver: 'broadcast',
      });

      // Should not throw and return assertions for chaining
      assertions
        .assertPublished('ISSUE_OPENED')
        .assertCount('ISSUE_OPENED', 1)
        .assertPublished('ANALYSIS_COMPLETE')
        .assertSequence(['ISSUE_OPENED', 'ANALYSIS_COMPLETE']);
    });
  });
}

function registerIsolationTests() {
  describe('isolation between clusters', function () {
    it('should only query messages from specified cluster', function () {
      ledger.append({
        cluster_id: 'cluster-a',
        topic: 'TEST',
        sender: 'agent-a',
        receiver: 'broadcast',
      });

      ledger.append({
        cluster_id: 'cluster-b',
        topic: 'TEST',
        sender: 'agent-b',
        receiver: 'broadcast',
      });

      const assertionsA = new LedgerAssertions(ledger, 'cluster-a');
      const assertionsB = new LedgerAssertions(ledger, 'cluster-b');

      assertionsA.assertCount('TEST', 1);
      assertionsB.assertCount('TEST', 1);

      // Both should have their own message
      const msgsA = assertionsA.getMessages('TEST');
      const msgsB = assertionsB.getMessages('TEST');

      assert.strictEqual(msgsA[0].sender, 'agent-a');
      assert.strictEqual(msgsB[0].sender, 'agent-b');
    });
  });
}
