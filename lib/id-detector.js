/**
 * ID Detector - Determines if an ID is a task or cluster
 *
 * Strategy:
 * 1. Check if ID exists in cluster storage
 * 2. If not, check if ID exists in task storage
 * 3. Return type: 'cluster', 'task', or null
 */

const path = require('path');
const fs = require('fs');
const os = require('os');

// Storage paths
const CLUSTER_DIR = path.join(os.homedir(), '.zeroshot');
const TASK_DIR = path.join(os.homedir(), '.claude-zeroshot');

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

  // Check tasks
  const taskFile = path.join(TASK_DIR, 'tasks.json');
  if (fs.existsSync(taskFile)) {
    try {
      const tasks = JSON.parse(fs.readFileSync(taskFile, 'utf8'));
      if (tasks[id]) {
        return 'task';
      }
    } catch {
      // Ignore parse errors
    }
  }

  return null;
}

module.exports = { detectIdType };
