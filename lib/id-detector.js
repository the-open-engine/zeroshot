/**
 * ID Detector - Determines if an ID is a task or cluster
 *
 * Strategy:
 * 1. Check if ID exists in cluster storage
 * 2. If not, check if ID exists in task storage (SQLite)
 * 3. Return type: 'cluster', 'task', or null
 */

const path = require('path');
const fs = require('fs');
const os = require('os');
const { tryLoadBetterSqlite3 } = require('./sqlite-runtime');

/**
 * Detect if ID is a cluster or task
 * @param {string} id - The ID to check
 * @returns {'cluster'|'task'|null} - Type of ID or null if not found
 */
function detectIdType(id) {
  const homeDir =
    process.env.ZEROSHOT_HOME || process.env.HOME || process.env.USERPROFILE || os.homedir();
  const clusterFile = path.join(homeDir, '.zeroshot', 'clusters.json');
  const taskDbFile = path.join(homeDir, '.claude-zeroshot', 'store.db');

  // Check clusters
  if (fs.existsSync(clusterFile)) {
    try {
      const clusters = JSON.parse(fs.readFileSync(clusterFile, 'utf8'));
      if (clusters[id]) {
        return 'cluster';
      }
    } catch {
      // Ignore parse errors
    }
  }

  // Check tasks in SQLite
  if (fs.existsSync(taskDbFile)) {
    const { Database } = tryLoadBetterSqlite3('task ID detection');
    if (!Database) {
      return null;
    }

    let db;
    try {
      db = new Database(taskDbFile, { readonly: true, timeout: 5000 });
      const row = db.prepare('SELECT id FROM tasks WHERE id = ?').get(id);
      if (row) {
        return 'task';
      }
    } catch {
      // Ignore DB lookup errors on read-only detection paths.
    } finally {
      if (db) {
        try {
          db.close();
        } catch {
          // Ignore close errors - database may already be closed
        }
      }
    }
  }

  return null;
}

module.exports = { detectIdType };
