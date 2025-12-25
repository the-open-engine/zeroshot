/**
 * LedgerAssertions - Test helper for validating ledger state
 *
 * Provides fluent assertion interface for testing message flow in clusters.
 * All methods support chaining for readable test specs.
 *
 * Usage:
 *   const assertions = new LedgerAssertions(ledger, clusterId);
 *   assertions
 *     .assertPublished('IMPLEMENTATION_READY')
 *     .assertCount('IMPLEMENTATION_READY', 1)
 *     .assertSequence(['ISSUE_OPENED', 'IMPLEMENTATION_READY', 'VALIDATION_RESULT']);
 */

const assert = require('assert');

class LedgerAssertions {
  /**
   * Create assertions helper bound to a cluster
   * @param {Ledger} ledger - Ledger instance
   * @param {string} clusterId - Cluster ID to assert on
   */
  constructor(ledger, clusterId) {
    if (!ledger) throw new Error('ledger is required');
    if (!clusterId) throw new Error('clusterId is required');

    this.ledger = ledger;
    this.clusterId = clusterId;
  }

  /**
   * Assert that at least one message was published on a topic
   * @param {string} topic - Topic to check
   * @param {Object} opts - Optional query filters
   * @param {string} opts.sender - Filter by sender (agent ID)
   * @param {number} opts.since - Filter by timestamp (milliseconds since epoch)
   * @param {number} opts.until - Filter by timestamp (milliseconds since epoch)
   * @returns {this} For chaining
   */
  assertPublished(topic, opts = {}) {
    assert(topic, 'topic is required');

    const messages = this.ledger.query({
      cluster_id: this.clusterId,
      topic,
      ...(opts.sender && { sender: opts.sender }),
      ...(opts.since && { since: opts.since }),
      ...(opts.until && { until: opts.until }),
    });

    assert(
      messages.length > 0,
      `Expected at least one message on topic "${topic}"${
        opts.sender ? ` from sender "${opts.sender}"` : ''
      }, but found none`
    );

    return this;
  }

  /**
   * Assert exact count of messages on a topic
   * @param {string} topic - Topic to check
   * @param {number} count - Expected message count
   * @returns {this} For chaining
   */
  assertCount(topic, count) {
    assert(topic, 'topic is required');
    assert(typeof count === 'number' && count >= 0, 'count must be a non-negative number');

    const actual = this.ledger.count({
      cluster_id: this.clusterId,
      topic,
    });

    assert.strictEqual(
      actual,
      count,
      `Expected ${count} messages on topic "${topic}", but found ${actual}`
    );

    return this;
  }

  /**
   * Assert topics appear in order (not necessarily consecutive)
   * @param {string[]} topics - Expected sequence of topics
   * @returns {this} For chaining
   */
  assertSequence(topics) {
    assert(Array.isArray(topics) && topics.length > 0, 'topics must be a non-empty array');

    const allMessages = this.ledger.getAll(this.clusterId);
    const allTopics = allMessages.map((m) => m.topic);

    // Build expected sequence (find indices where each topic appears)
    let lastFoundIndex = -1;
    const foundIndices = [];

    for (const expectedTopic of topics) {
      const foundIndex = allTopics.findIndex((t, i) => i > lastFoundIndex && t === expectedTopic);

      if (foundIndex === -1) {
        // Topic not found after previous topic
        const actualStr = allTopics.length > 0 ? allTopics.join(' → ') : '(no messages)';

        assert.fail(
          `Expected topic sequence: ${topics.join(' → ')}\n` +
            `Actual sequence: ${actualStr}\n` +
            `Missing topic "${expectedTopic}" after position ${lastFoundIndex}`
        );
      }

      foundIndices.push(foundIndex);
      lastFoundIndex = foundIndex;
    }

    return this;
  }

  /**
   * Get the last message on a topic
   * @param {string} topic - Topic to query
   * @returns {Object|null} Last message or null if not found
   */
  lastMessage(topic) {
    assert(topic, 'topic is required');

    return this.ledger.findLast({
      cluster_id: this.clusterId,
      topic,
    });
  }

  /**
   * Get all messages on a topic
   * @param {string} topic - Topic to query
   * @returns {Object[]} Array of messages
   */
  getMessages(topic) {
    assert(topic, 'topic is required');

    return this.ledger.query({
      cluster_id: this.clusterId,
      topic,
    });
  }
}

module.exports = LedgerAssertions;
