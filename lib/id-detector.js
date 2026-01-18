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

// Storage paths
const CLUSTER_DIR = path.join(os.homedir(), '.zeroshot');
const TASK_DIR = path.join(os.homedir(), '.claude-zeroshot');
const TASK_DB_FILE = path.join(TASK_DIR, 'store.db');

/**
 * Detect if ID is a cluster or task
 * @param {string} id - The ID to check
 * @returns {'cluster'|'task'|null} - Type of ID or null if not found
 */
function detectIdType(id) {
  // Check clusters
  const clusterFile = path.join(CLUSTER_DIR, 'clusters.json');
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
  if (fs.existsSync(TASK_DB_FILE)) {
    try {
      const db = new Database(TASK_DB_FILE, { readonly: true, timeout: 5000 });
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
