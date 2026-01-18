/**
 * DataPoller - Aggregates cluster data for TUI display
 *
 * Polls all data sources at appropriate intervals:
 * - Cluster states (1s)
 * - Resource stats via pidusage (2s)
 * - New cluster detection (2s)
 * - Ledger message streaming (500ms per cluster)
 */

const pidusage = require('pidusage');
const Ledger = require('../ledger');
const fs = require('fs');
const path = require('path');
const os = require('os');

class DataPoller {
  constructor(orchestrator, options = {}) {
    this.orchestrator = orchestrator;
    this.intervals = [];
    this.ledgers = new Map(); // clusterId -> Ledger instance
    this.ledgerStopFns = new Map(); // clusterId -> stop function for pollForMessages
    this.onUpdate = options.onUpdate || (() => {}); // Callback for updates
    this.watchForNewClustersStopFn = null;
  }

  /**
   * Start all polling intervals
   */
  start() {
    // Poll cluster states (1s)
    const clusterStateInterval = setInterval(() => {
      this._pollClusterStates();
    }, 1000);
    this.intervals.push(clusterStateInterval);

    // Poll resource stats (2s)
    const resourceStatsInterval = setInterval(() => {
      this._pollResourceStats();
    }, 2000);
    this.intervals.push(resourceStatsInterval);

    // Watch for new clusters (2s)
    this._watchForNewClusters();

    // Defer initial polls to avoid blocking UI startup
    // Run in background after 50ms to let UI render first
    setTimeout(() => {
      this._pollClusterStates();
    }, 50);

    setTimeout(() => {
      this._pollResourceStats();
    }, 100);
  }

  /**
   * Stop all polling intervals and clean up resources
   */
  stop() {
    // Clear all intervals
    for (const intervalId of this.intervals) {
      clearInterval(intervalId);
    }
    this.intervals = [];

    // Stop watching for new clusters
    if (this.watchForNewClustersStopFn) {
      this.watchForNewClustersStopFn();
      this.watchForNewClustersStopFn = null;
    }

    // Stop all ledger polling
    for (const stopFn of this.ledgerStopFns.values()) {
      stopFn();
    }
    this.ledgerStopFns.clear();

    // Close all ledger connections
    for (const ledger of this.ledgers.values()) {
      try {
        ledger.close();
      } catch {
        // Ignore errors during cleanup
      }
    }
    this.ledgers.clear();
  }

  /**
   * Poll cluster states (1s interval)
   * Gets all clusters and their agent states from orchestrator
   * @private
   */
  _pollClusterStates() {
    try {
      const clusters = this.orchestrator.listClusters();

      // Get detailed status for each cluster
      const clustersWithStatus = clusters.map((cluster) => {
        try {
          const status = this.orchestrator.getStatus(cluster.id);
          // Add agentCount for stats calculation
          return {
            ...status,
            agentCount: status.agents ? status.agents.length : 0,
          };
        } catch (error) {
          console.error(
            `[DataPoller] Failed to get status for cluster ${cluster.id}:`,
            error.message
          );
          return {
            id: cluster.id,
            state: 'unknown',
            createdAt: cluster.createdAt,
            agents: [],
            agentCount: 0,
            messageCount: 0,
          };
        }
      });

      this.onUpdate({
        type: 'cluster_state',
        clusters: clustersWithStatus,
      });
    } catch (error) {
      console.error('[DataPoller] _pollClusterStates error:', error.message);
    }
  }

  /**
   * Poll resource stats (2s interval)
   * Uses pidusage to get CPU and memory for all agent processes
   * @private
   */
  async _pollResourceStats() {
    try {
      const clusters = this.orchestrator.listClusters();
      const stats = {};
      const pids = this._collectClusterPids(clusters);

      if (pids.length > 0) {
        const pidStats = await this._safePidUsage(pids);
        this._populatePidStats(stats, pids, pidStats);
      }

      this.onUpdate({
        type: 'resource_stats',
        stats,
      });
    } catch (error) {
      console.error('[DataPoller] _pollResourceStats error:', error.message);
    }
  }

  /**
   * Watch for new clusters (2s interval)
   * Uses orchestrator.watchForNewClusters to detect new clusters
   * and start streaming their ledger messages
   * @private
   */
  _watchForNewClusters() {
    this.watchForNewClustersStopFn = this.orchestrator.watchForNewClusters((cluster) => {
      // Lazy load ledger only when we need to stream messages
      // This avoids loading all ledgers on startup
      const ledger = this._loadLedgerForCluster(cluster, {
        label: 'cluster',
        requireExisting: true,
      });

      if (!ledger) {
        return;
      }

      // Start streaming messages
      this._streamLedgerMessages(cluster.id);

      // Emit update about new cluster
      this.onUpdate({
        type: 'new_cluster',
        cluster,
      });
    }, 2000);

    // Also load ledgers for all existing clusters
    const existingClusters = this.orchestrator.listClusters();
    for (const cluster of existingClusters) {
      const ledger = this._loadLedgerForCluster(cluster, { label: 'existing cluster' });
      if (!ledger) {
        continue;
      }

      this._streamLedgerMessages(cluster.id);
    }
  }

  /**
   * Stream ledger messages for a cluster (500ms interval)
   * Uses ledger.pollForMessages to get new messages
   * @param {string} clusterId - Cluster ID to stream messages from
   * @private
   */
  _streamLedgerMessages(clusterId) {
    const ledger = this.ledgers.get(clusterId);
    if (!ledger) {
      console.error(`[DataPoller] No ledger found for cluster ${clusterId}`);
      return;
    }

    // Stop existing polling if any
    const existingStopFn = this.ledgerStopFns.get(clusterId);
    if (existingStopFn) {
      existingStopFn();
    }

    // Start polling for messages
    const stopFn = ledger.pollForMessages(
      clusterId,
      (message) => {
        this.onUpdate({
          type: 'new_message',
          clusterId,
          message,
        });
      },
      500, // Poll every 500ms
      50 // Show last 50 messages initially
    );

    this.ledgerStopFns.set(clusterId, stopFn);
  }

  /**
   * Collect resource stats for all agent PIDs
   * @returns {Object} Map of pid -> { cpu, memory }
   * @private
   */
  async _collectResourceStats() {
    const stats = {};
    const clusters = this.orchestrator.listClusters();

    for (const cluster of clusters) {
      const pids = this._getClusterAgentPids(cluster);
      for (const pid of pids) {
        stats[pid] = await this._getSinglePidStat(pid);
      }
    }

    return stats;
  }

  _collectClusterPids(clusters) {
    const pids = [];

    for (const cluster of clusters) {
      pids.push(...this._getClusterAgentPids(cluster));
    }

    return pids;
  }

  _getClusterAgentPids(cluster) {
    try {
      const status = this.orchestrator.getStatus(cluster.id);
      const pids = [];

      for (const agent of status.agents || []) {
        if (agent.pid) {
          pids.push(agent.pid);
        }
      }

      return pids;
    } catch {
      // Skip clusters that error
      return [];
    }
  }

  async _getSinglePidStat(pid) {
    try {
      const pidStat = await pidusage(pid);
      return {
        cpu: pidStat.cpu || 0,
        memory: pidStat.memory || 0,
      };
    } catch {
      // Process died - set to zero
      return { cpu: 0, memory: 0 };
    }
  }

  async _safePidUsage(pids) {
    try {
      return await pidusage(pids);
    } catch {
      return null;
    }
  }

  _populatePidStats(stats, pids, pidStats) {
    for (const pid of pids) {
      const entry = pidStats?.[pid];
      stats[pid] = {
        cpu: entry?.cpu || 0,
        memory: entry?.memory || 0,
      };
    }
  }

  _loadLedgerForCluster(cluster, options) {
    const { label, requireExisting = false } = options;

    try {
      return this._ensureLedger(cluster.id, { requireExisting });
    } catch (error) {
      console.error(
        `[DataPoller] Failed to load ledger for ${label} ${cluster.id}:`,
        error.message
      );
      return null;
    }
  }

  _ensureLedger(clusterId, { requireExisting = false } = {}) {
    const existing = this.ledgers.get(clusterId);
    if (existing) {
      return existing;
    }

    const dbPath = this._getLedgerPath(clusterId);
    if (requireExisting && !fs.existsSync(dbPath)) {
      return null;
    }

    const ledger = new Ledger(dbPath);
    this.ledgers.set(clusterId, ledger);
    return ledger;
  }

  _getLedgerPath(clusterId) {
    const storageDir = this.orchestrator.storageDir || path.join(os.homedir(), '.zeroshot');
    return path.join(storageDir, `${clusterId}.db`);
  }
}

module.exports = DataPoller;
