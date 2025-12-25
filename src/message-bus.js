/**
 * MessageBus - Pub/sub layer over Ledger with WebSocket support
 *
 * Provides:
 * - High-level publish/subscribe API
 * - WebSocket broadcasting for UI clients
 * - Topic-based routing
 * - Real-time event distribution
 */

const EventEmitter = require('events');
const Ledger = require('./ledger');

// Expected listeners per cluster:
// - 5 orchestrator subscriptions (CLUSTER_COMPLETE, CLUSTER_FAILED, AGENT_ERROR, AGENT_LIFECYCLE, CLUSTER_OPERATIONS)
// - 1 ledger internal
// - 1 per agent (can be 10+ with dynamic spawning)
// - topic-specific listeners
const MAX_LISTENERS = 50;

class MessageBus extends EventEmitter {
  constructor(ledger) {
    super();
    this.setMaxListeners(MAX_LISTENERS);
    this.ledger = ledger || new Ledger();
    this.wsClients = new Set();

    // Forward ledger events
    this.ledger.on('message', (message) => {
      this.emit('message', message);
      this._broadcastToWebSocket(message);
    });
  }

  /**
   * Publish a message to the ledger
   * @param {Object} message - Message to publish
   * @returns {Object} Published message with ID
   */
  publish(message) {
    if (!message.cluster_id) {
      throw new Error('cluster_id is required');
    }

    if (!message.topic) {
      throw new Error('topic is required');
    }

    if (!message.sender) {
      throw new Error('sender is required');
    }

    const published = this.ledger.append(message);

    // Emit to topic-specific listeners
    this.emit(`topic:${message.topic}`, published);

    return published;
  }

  /**
   * Subscribe to all messages
   * @param {Function} callback - Called with each message
   * @returns {Function} Unsubscribe function
   */
  subscribe(callback) {
    this.on('message', callback);
    return () => this.off('message', callback);
  }

  /**
   * Subscribe to specific topic
   * @param {String} topic - Topic to subscribe to
   * @param {Function} callback - Called with matching messages
   * @returns {Function} Unsubscribe function
   */
  subscribeTopic(topic, callback) {
    const event = `topic:${topic}`;
    this.on(event, callback);
    return () => this.off(event, callback);
  }

  /**
   * Subscribe to multiple topics
   * @param {Array<String>} topics - Topics to subscribe to
   * @param {Function} callback - Called with matching messages
   * @returns {Function} Unsubscribe function
   */
  subscribeTopics(topics, callback) {
    const unsubscribers = topics.map((topic) => this.subscribeTopic(topic, callback));
    return () => unsubscribers.forEach((unsub) => unsub());
  }

  /**
   * Query messages (passthrough to ledger)
   */
  query(criteria) {
    return this.ledger.query(criteria);
  }

  /**
   * Find last message (passthrough to ledger)
   */
  findLast(criteria) {
    return this.ledger.findLast(criteria);
  }

  /**
   * Count messages (passthrough to ledger)
   */
  count(criteria) {
    return this.ledger.count(criteria);
  }

  /**
   * Get messages since timestamp (passthrough to ledger)
   */
  since(params) {
    return this.ledger.since(params);
  }

  /**
   * Get all messages (passthrough to ledger)
   */
  getAll(cluster_id) {
    return this.ledger.getAll(cluster_id);
  }

  /**
   * Register a WebSocket client for broadcasts
   * @param {WebSocket} ws - WebSocket connection
   */
  addWebSocketClient(ws) {
    this.wsClients.add(ws);

    ws.on('close', () => {
      this.wsClients.delete(ws);
    });

    ws.on('error', () => {
      this.wsClients.delete(ws);
    });
  }

  /**
   * Remove a WebSocket client
   * @param {WebSocket} ws - WebSocket connection
   */
  removeWebSocketClient(ws) {
    this.wsClients.delete(ws);
  }

  /**
   * Broadcast message to all WebSocket clients
   * @private
   */
  _broadcastToWebSocket(message) {
    const payload = JSON.stringify({
      type: 'message',
      data: message,
    });

    for (const ws of this.wsClients) {
      if (ws.readyState === 1) {
        // OPEN
        try {
          ws.send(payload);
        } catch (error) {
          console.error('WebSocket send error:', error);
          this.wsClients.delete(ws);
        }
      }
    }
  }

  /**
   * Close the message bus
   */
  close() {
    // Close all WebSocket connections
    for (const ws of this.wsClients) {
      try {
        ws.close();
      } catch {
        // Ignore errors on close
      }
    }
    this.wsClients.clear();

    // Close ledger
    this.ledger.close();
  }

  /**
   * Clear all messages (for testing)
   */
  clear() {
    this.ledger.clear();
  }
}

module.exports = MessageBus;
