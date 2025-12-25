/**
 * Ledger - Immutable event log for multi-agent coordination
 *
 * Provides:
 * - SQLite-backed message storage with indexes
 * - Query API for message retrieval
 * - In-memory cache for recent queries
 * - Subscription mechanism for real-time updates
 */

const Database = require('better-sqlite3');
const EventEmitter = require('events');
const crypto = require('crypto');

class Ledger extends EventEmitter {
  constructor(dbPath = ':memory:') {
    super();
    this.db = new Database(dbPath);
    this.cache = new Map(); // LRU cache for queries
    this.cacheLimit = 1000;
    this._initSchema();
  }

  _initSchema() {
    // Enable WAL mode for concurrent reads
    this.db.pragma('journal_mode = WAL');
    // Force synchronous writes so other processes see changes immediately
    this.db.pragma('synchronous = NORMAL');
    // Checkpoint WAL frequently for cross-process visibility
    this.db.pragma('wal_autocheckpoint = 1');

    // Create messages table
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS messages (
        id TEXT PRIMARY KEY,
        timestamp INTEGER NOT NULL,
        topic TEXT NOT NULL,
        sender TEXT NOT NULL,
        receiver TEXT NOT NULL,
        content_text TEXT,
        content_data TEXT,
        metadata TEXT,
        cluster_id TEXT NOT NULL
      );

      CREATE INDEX IF NOT EXISTS idx_timestamp ON messages(timestamp);
      CREATE INDEX IF NOT EXISTS idx_topic ON messages(topic);
      CREATE INDEX IF NOT EXISTS idx_cluster_sender ON messages(cluster_id, sender);
      CREATE INDEX IF NOT EXISTS idx_cluster_topic ON messages(cluster_id, topic);
      CREATE INDEX IF NOT EXISTS idx_cluster_timestamp ON messages(cluster_id, timestamp);
    `);

    this._prepareStatements();
  }

  _prepareStatements() {
    this.stmts = {
      insert: this.db.prepare(`
        INSERT INTO messages (id, timestamp, topic, sender, receiver, content_text, content_data, metadata, cluster_id)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
      `),

      queryBase: `SELECT * FROM messages WHERE cluster_id = ?`,

      count: this.db.prepare(`SELECT COUNT(*) as count FROM messages WHERE cluster_id = ?`),

      getAll: this.db.prepare(`SELECT * FROM messages WHERE cluster_id = ? ORDER BY timestamp ASC`),
    };
  }

  /**
   * Append a message to the ledger
   * @param {Object} message - Message object
   * @returns {Object} The appended message with generated ID
   */
  append(message) {
    const id = message.id || `msg_${crypto.randomBytes(16).toString('hex')}`;
    const timestamp = message.timestamp || Date.now();

    const record = {
      id,
      timestamp,
      topic: message.topic,
      sender: message.sender,
      receiver: message.receiver || 'broadcast',
      content_text: message.content?.text || null,
      content_data: message.content?.data ? JSON.stringify(message.content.data) : null,
      metadata: message.metadata ? JSON.stringify(message.metadata) : null,
      cluster_id: message.cluster_id,
    };

    try {
      this.stmts.insert.run(
        record.id,
        record.timestamp,
        record.topic,
        record.sender,
        record.receiver,
        record.content_text,
        record.content_data,
        record.metadata,
        record.cluster_id
      );

      // Invalidate cache
      this.cache.clear();

      // Emit event for subscriptions
      const fullMessage = this._deserializeMessage(record);
      this.emit('message', fullMessage);
      this.emit(`topic:${message.topic}`, fullMessage);

      return fullMessage;
    } catch (error) {
      throw new Error(`Failed to append message: ${error.message}`);
    }
  }

  /**
   * Query messages with filters
   * @param {Object} criteria - Query criteria
   * @returns {Array} Matching messages
   */
  query(criteria) {
    const { cluster_id, topic, sender, receiver, since, until, limit, offset } = criteria;

    if (!cluster_id) {
      throw new Error('cluster_id is required for queries');
    }

    // Build query
    const conditions = ['cluster_id = ?'];
    const params = [cluster_id];

    if (topic) {
      conditions.push('topic = ?');
      params.push(topic);
    }

    if (sender) {
      conditions.push('sender = ?');
      params.push(sender);
    }

    if (receiver) {
      conditions.push('receiver = ?');
      params.push(receiver);
    }

    if (since) {
      conditions.push('timestamp >= ?');
      params.push(typeof since === 'number' ? since : new Date(since).getTime());
    }

    if (until) {
      conditions.push('timestamp <= ?');
      params.push(typeof until === 'number' ? until : new Date(until).getTime());
    }

    let sql = `SELECT * FROM messages WHERE ${conditions.join(' AND ')} ORDER BY timestamp ASC`;

    if (limit) {
      sql += ` LIMIT ?`;
      params.push(limit);
    }

    if (offset) {
      sql += ` OFFSET ?`;
      params.push(offset);
    }

    const stmt = this.db.prepare(sql);
    const rows = stmt.all(...params);
    return rows.map((row) => this._deserializeMessage(row));
  }

  /**
   * Find the last message matching criteria
   * @param {Object} criteria - Query criteria
   * @returns {Object|null} Last matching message
   */
  findLast(criteria) {
    const { cluster_id, topic, sender, receiver, since, until } = criteria;

    if (!cluster_id) {
      throw new Error('cluster_id is required for queries');
    }

    // Build query with DESC order
    const conditions = ['cluster_id = ?'];
    const params = [cluster_id];

    if (topic) {
      conditions.push('topic = ?');
      params.push(topic);
    }

    if (sender) {
      conditions.push('sender = ?');
      params.push(sender);
    }

    if (receiver) {
      conditions.push('receiver = ?');
      params.push(receiver);
    }

    if (since) {
      conditions.push('timestamp >= ?');
      params.push(typeof since === 'number' ? since : new Date(since).getTime());
    }

    if (until) {
      conditions.push('timestamp <= ?');
      params.push(typeof until === 'number' ? until : new Date(until).getTime());
    }

    const sql = `SELECT * FROM messages WHERE ${conditions.join(' AND ')} ORDER BY timestamp DESC LIMIT 1`;

    const stmt = this.db.prepare(sql);
    const row = stmt.get(...params);
    return row ? this._deserializeMessage(row) : null;
  }

  /**
   * Count messages matching criteria
   * @param {Object} criteria - Query criteria
   * @returns {Number} Message count
   */
  count(criteria) {
    const { cluster_id, topic } = criteria;

    if (!cluster_id) {
      throw new Error('cluster_id is required for count');
    }

    let sql = 'SELECT COUNT(*) as count FROM messages WHERE cluster_id = ?';
    const params = [cluster_id];

    if (topic) {
      sql += ' AND topic = ?';
      params.push(topic);
    }

    const stmt = this.db.prepare(sql);
    const result = stmt.get(...params);
    return result.count;
  }

  /**
   * Get messages since a specific timestamp
   * @param {Object} params - { cluster_id, timestamp }
   * @returns {Array} Messages since timestamp
   */
  since(params) {
    return this.query({
      cluster_id: params.cluster_id,
      since: params.timestamp,
    });
  }

  /**
   * Get all messages for a cluster
   * @param {String} cluster_id - Cluster ID
   * @returns {Array} All messages
   */
  getAll(cluster_id) {
    const rows = this.stmts.getAll.all(cluster_id);
    return rows.map((row) => this._deserializeMessage(row));
  }

  /**
   * Subscribe to new messages
   * @param {Function} callback - Called with each new message
   * @returns {Function} Unsubscribe function
   */
  subscribe(callback) {
    this.on('message', callback);
    return () => this.off('message', callback);
  }

  /**
   * Poll for new messages (cross-process support)
   * @param {String} clusterId - Cluster ID to poll (null for all clusters)
   * @param {Function} callback - Called with each new message
   * @param {Number} intervalMs - Poll interval (default 500ms)
   * @param {Number} initialCount - Number of messages to show initially (default 300)
   * @returns {Function} Stop polling function
   */
  pollForMessages(clusterId, callback, intervalMs = 500, initialCount = 300) {
    let lastTimestamp = 0;
    let lastMessageIds = new Set();
    let isFirstPoll = true;

    const poll = () => {
      try {
        let sql, params;

        if (isFirstPoll) {
          // First poll: get last N messages by count
          if (clusterId) {
            sql =
              'SELECT * FROM (SELECT * FROM messages WHERE cluster_id = ? ORDER BY timestamp DESC LIMIT ?) ORDER BY timestamp ASC';
            params = [clusterId, initialCount];
          } else {
            sql =
              'SELECT * FROM (SELECT * FROM messages ORDER BY timestamp DESC LIMIT ?) ORDER BY timestamp ASC';
            params = [initialCount];
          }
          isFirstPoll = false;
        } else {
          // Subsequent polls: get messages since last timestamp
          if (clusterId) {
            sql =
              'SELECT * FROM messages WHERE cluster_id = ? AND timestamp >= ? ORDER BY timestamp ASC';
            params = [clusterId, lastTimestamp - 1000]; // 1s buffer for race conditions
          } else {
            sql = 'SELECT * FROM messages WHERE timestamp >= ? ORDER BY timestamp ASC';
            params = [lastTimestamp - 1000];
          }
        }

        const stmt = this.db.prepare(sql);
        const rows = stmt.all(...params);

        for (const row of rows) {
          // Skip already-seen messages
          if (lastMessageIds.has(row.id)) continue;

          lastMessageIds.add(row.id);
          const message = this._deserializeMessage(row);
          callback(message);

          // Update timestamp high-water mark
          if (row.timestamp > lastTimestamp) {
            lastTimestamp = row.timestamp;
          }
        }

        // Prune old message IDs to prevent memory leak
        if (lastMessageIds.size > 10000) {
          const idsArray = Array.from(lastMessageIds);
          lastMessageIds = new Set(idsArray.slice(-5000));
        }
      } catch (error) {
        // DB busy is expected during concurrent access - log but continue polling
        // Other errors indicate real bugs and should be visible
        console.error(`[Ledger] pollForMessages error (will retry): ${error.message}`);
      }
    };

    // Initial poll
    poll();

    // Set up interval
    const intervalId = setInterval(poll, intervalMs);

    // Return stop function
    return () => clearInterval(intervalId);
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
   * Deserialize a database row into a message object
   * @private
   */
  _deserializeMessage(row) {
    const message = {
      id: row.id,
      timestamp: row.timestamp,
      topic: row.topic,
      sender: row.sender,
      receiver: row.receiver,
      cluster_id: row.cluster_id,
    };

    if (row.content_text || row.content_data) {
      message.content = {};
      if (row.content_text) {
        message.content.text = row.content_text;
      }
      if (row.content_data) {
        try {
          message.content.data = JSON.parse(row.content_data);
        } catch {
          message.content.data = null;
        }
      }
    }

    if (row.metadata) {
      try {
        message.metadata = JSON.parse(row.metadata);
      } catch {
        message.metadata = null;
      }
    }

    return message;
  }

  /**
   * Close the database connection
   */
  close() {
    this.db.close();
  }

  /**
   * Clear all messages (for testing)
   */
  clear() {
    this.db.exec('DELETE FROM messages');
    this.cache.clear();
  }
}

module.exports = Ledger;
