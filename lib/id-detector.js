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
const Database = require('better-sqlite3');
const { readClustersFileSync } = require('./clusters-registry');

/**
 * Detect if ID is a cluster or task
 * @param {string} id - The ID to check
 * @returns {'cluster'|'task'|null} - Type of ID or null if not found
 */
function detectIdType(id) {
  const homeDir =
    process.env.ZEROSHOT_HOME || process.env.HOME || process.env.USERPROFILE || os.homedir();
  const storageDir = path.join(homeDir, '.zeroshot');
  const taskDbFile = path.join(homeDir, '.claude-zeroshot', 'store.db');

  // Check clusters
  try {
    const clusters = readClustersFileSync(storageDir);
    if (clusters[id]) {
      return 'cluster';
    }
  } catch {
    // Ignore parse errors
  }

  // Check tasks in SQLite
  if (fs.existsSync(taskDbFile)) {
    try {
      const db = new Database(taskDbFile, { readonly: true, timeout: 5000 });
      const row = db.prepare('SELECT id FROM tasks WHERE id = ?').get(id);
      db.close();
      if (row) {
        return 'task';
      }
    } catch {
      // Ignore DB errors
    }
  }

  return null;
}

module.exports = { detectIdType };
