/**
 * Socket Discovery - Utilities for socket path management
 *
 * Socket locations:
 * - Tasks: ~/.zeroshot/sockets/task-<id>.sock
 * - Clusters: ~/.zeroshot/sockets/cluster-<id>.sock (cluster-level, future)
 * - Agents: ~/.zeroshot/sockets/cluster-<id>/<agent-id>.sock
 */

const path = require('path');
const fs = require('fs');
const os = require('os');
const net = require('net');

const CREW_DIR = path.join(os.homedir(), '.zeroshot');
const SOCKET_DIR = path.join(CREW_DIR, 'sockets');

/**
 * Ensure socket directory exists
 */
function ensureSocketDir() {
  if (!fs.existsSync(SOCKET_DIR)) {
    fs.mkdirSync(SOCKET_DIR, { recursive: true });
  }
}

/**
 * Get socket path for a task
 * @param {string} taskId - Task ID (e.g., 'task-swift-falcon')
 * @returns {string} - Socket path
 */
function getTaskSocketPath(taskId) {
  ensureSocketDir();
  return path.join(SOCKET_DIR, `${taskId}.sock`);
}

/**
 * Get socket path for a cluster agent
 * @param {string} clusterId - Cluster ID (e.g., 'cluster-bold-eagle')
 * @param {string} agentId - Agent ID (e.g., 'worker')
 * @returns {string} - Socket path
 */
function getAgentSocketPath(clusterId, agentId) {
  const clusterDir = path.join(SOCKET_DIR, clusterId);
  if (!fs.existsSync(clusterDir)) {
    fs.mkdirSync(clusterDir, { recursive: true });
  }
  return path.join(clusterDir, `${agentId}.sock`);
}

/**
 * Get socket path for any ID (auto-detects task vs cluster)
 * @param {string} id - Task or cluster ID
 * @param {string} [agentId] - Optional agent ID for clusters
 * @returns {string} - Socket path
 */
function getSocketPath(id, agentId = null) {
  if (id.startsWith('task-')) {
    return getTaskSocketPath(id);
  }
  if (id.startsWith('cluster-')) {
    if (agentId) {
      return getAgentSocketPath(id, agentId);
    }
    // Cluster-level socket (future use)
    ensureSocketDir();
    return path.join(SOCKET_DIR, `${id}.sock`);
  }
  // Unknown format, treat as task
  return getTaskSocketPath(id);
}

/**
 * Check if a socket exists and is connectable
 * @param {string} socketPath - Path to socket file
 * @returns {Promise<boolean>} - True if socket is live
 */
function isSocketAlive(socketPath) {
  if (!fs.existsSync(socketPath)) {
    return Promise.resolve(false);
  }

  return new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    const timeout = setTimeout(() => {
      socket.destroy();
      resolve(false);
    }, 1000);

    socket.on('connect', () => {
      clearTimeout(timeout);
      socket.end();
      resolve(true);
    });

    socket.on('error', () => {
      clearTimeout(timeout);
      resolve(false);
    });
  });
}

/**
 * Remove stale socket file if not connectable
 * @param {string} socketPath - Path to socket file
 * @returns {Promise<boolean>} - True if socket was removed (stale)
 */
async function cleanupStaleSocket(socketPath) {
  if (!fs.existsSync(socketPath)) {
    return false;
  }

  const alive = await isSocketAlive(socketPath);
  if (!alive) {
    try {
      fs.unlinkSync(socketPath);
      return true;
    } catch {
      // Ignore errors (file may have been removed already)
    }
  }
  return false;
}

/**
 * List all attachable tasks
 * Task sockets are .sock files directly in SOCKET_DIR (not in subdirectories)
 * Excludes cluster-level sockets (cluster-xxx.sock) since those aren't tasks
 * @returns {Promise<string[]>} - Array of task IDs with live sockets
 */
async function listAttachableTasks() {
  ensureSocketDir();
  const entries = fs.readdirSync(SOCKET_DIR, { withFileTypes: true });
  const tasks = [];

  for (const entry of entries) {
    // Check socket files (Unix sockets report isSocket(), not isFile())
    // Also accept regular files for compatibility
    const isSocketFile = (entry.isSocket() || entry.isFile()) && entry.name.endsWith('.sock');
    if (isSocketFile && !entry.isDirectory()) {
      const id = entry.name.slice(0, -5); // Remove .sock

      // Skip cluster-level sockets (cluster-xxx.sock)
      if (id.startsWith('cluster-')) {
        continue;
      }

      const socketPath = path.join(SOCKET_DIR, entry.name);
      if (await isSocketAlive(socketPath)) {
        tasks.push(id);
      }
    }
  }

  return tasks;
}

/**
 * List all attachable agents for a cluster
 * @param {string} clusterId - Cluster ID
 * @returns {Promise<string[]>} - Array of agent IDs with live sockets
 */
async function listAttachableAgents(clusterId) {
  const clusterDir = path.join(SOCKET_DIR, clusterId);
  if (!fs.existsSync(clusterDir)) {
    return [];
  }

  const files = fs.readdirSync(clusterDir);
  const agents = [];

  for (const file of files) {
    if (file.endsWith('.sock')) {
      const agentId = file.slice(0, -5); // Remove .sock
      const socketPath = path.join(clusterDir, file);
      if (await isSocketAlive(socketPath)) {
        agents.push(agentId);
      }
    }
  }

  return agents;
}

/**
 * List all attachable clusters
 * @returns {Promise<string[]>} - Array of cluster IDs with at least one live agent socket
 */
async function listAttachableClusters() {
  ensureSocketDir();
  const entries = fs.readdirSync(SOCKET_DIR, { withFileTypes: true });
  const clusters = [];

  for (const entry of entries) {
    if (entry.isDirectory() && entry.name.startsWith('cluster-')) {
      const agents = await listAttachableAgents(entry.name);
      if (agents.length > 0) {
        clusters.push(entry.name);
      }
    }
  }

  return clusters;
}

/**
 * Cleanup all sockets for a cluster (on cluster stop)
 * @param {string} clusterId - Cluster ID
 */
function cleanupClusterSockets(clusterId) {
  const clusterDir = path.join(SOCKET_DIR, clusterId);
  if (fs.existsSync(clusterDir)) {
    const files = fs.readdirSync(clusterDir);
    for (const file of files) {
      try {
        fs.unlinkSync(path.join(clusterDir, file));
      } catch {
        // Ignore
      }
    }
    try {
      fs.rmdirSync(clusterDir);
    } catch {
      // Ignore
    }
  }
}

module.exports = {
  CREW_DIR,
  SOCKET_DIR,
  ensureSocketDir,
  getTaskSocketPath,
  getAgentSocketPath,
  getSocketPath,
  isSocketAlive,
  cleanupStaleSocket,
  listAttachableTasks,
  listAttachableAgents,
  listAttachableClusters,
  cleanupClusterSockets,
};
