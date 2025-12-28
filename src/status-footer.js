/**
 * StatusFooter - Persistent terminal status bar for live agent monitoring
 *
 * Displays:
 * - Agent status icons (🟢 running, ⏳ waiting, 🔄 processing, etc.)
 * - Real-time CPU, memory, network metrics per agent
 * - Cluster summary stats
 *
 * Uses ANSI escape sequences to maintain a fixed footer while
 * allowing normal terminal output to scroll above it.
 */

const { getProcessMetrics, formatMetrics, getStateIcon } = require('./process-metrics');

// ANSI escape codes
const ESC = '\x1b';
const CSI = `${ESC}[`;

// Terminal manipulation
const SAVE_CURSOR = `${CSI}s`;
const RESTORE_CURSOR = `${CSI}u`;
const CLEAR_LINE = `${CSI}2K`;
const HIDE_CURSOR = `${CSI}?25l`;
const SHOW_CURSOR = `${CSI}?25h`;

// Colors
const COLORS = {
  reset: `${CSI}0m`,
  bold: `${CSI}1m`,
  dim: `${CSI}2m`,
  cyan: `${CSI}36m`,
  green: `${CSI}32m`,
  yellow: `${CSI}33m`,
  red: `${CSI}31m`,
  gray: `${CSI}90m`,
  white: `${CSI}37m`,
  bgBlack: `${CSI}40m`,
};

/**
 * @typedef {Object} AgentState
 * @property {string} id - Agent ID
 * @property {string} state - Agent state (idle, executing, etc.)
 * @property {number|null} pid - Process ID if running
 * @property {number} iteration - Current iteration
 */

class StatusFooter {
  /**
   * @param {Object} options
   * @param {number} [options.refreshInterval=1000] - Refresh interval in ms
   * @param {boolean} [options.enabled=true] - Whether footer is enabled
   */
  constructor(options = {}) {
    this.refreshInterval = options.refreshInterval || 1000;
    this.enabled = options.enabled !== false;
    this.intervalId = null;
    this.agents = new Map(); // agentId -> AgentState
    this.metricsCache = new Map(); // agentId -> ProcessMetrics
    this.footerHeight = 2; // Lines reserved for footer
    this.scrollRegionSet = false;
    this.clusterId = null;
    this.clusterState = 'initializing';
    this.startTime = Date.now();
  }

  /**
   * Check if we're in a TTY that supports the footer
   * @returns {boolean}
   */
  isTTY() {
    return process.stdout.isTTY === true;
  }

  /**
   * Get terminal dimensions
   * @returns {{ rows: number, cols: number }}
   */
  getTerminalSize() {
    return {
      rows: process.stdout.rows || 24,
      cols: process.stdout.columns || 80,
    };
  }

  /**
   * Move cursor to specific position
   * @param {number} row - 1-based row
   * @param {number} col - 1-based column
   */
  moveTo(row, col) {
    process.stdout.write(`${CSI}${row};${col}H`);
  }

  /**
   * Set up scroll region to reserve space for footer
   */
  setupScrollRegion() {
    if (!this.isTTY()) return;

    const { rows } = this.getTerminalSize();
    const scrollEnd = rows - this.footerHeight;

    // Set scroll region (lines 1 to scrollEnd)
    process.stdout.write(`${CSI}1;${scrollEnd}r`);

    // Move cursor to top of scroll region
    process.stdout.write(`${CSI}1;1H`);

    this.scrollRegionSet = true;
  }

  /**
   * Reset scroll region to full terminal
   */
  resetScrollRegion() {
    if (!this.isTTY()) return;

    const { rows } = this.getTerminalSize();
    process.stdout.write(`${CSI}1;${rows}r`);
    this.scrollRegionSet = false;
  }

  /**
   * Register cluster for monitoring
   * @param {string} clusterId
   */
  setCluster(clusterId) {
    this.clusterId = clusterId;
  }

  /**
   * Update cluster state
   * @param {string} state
   */
  setClusterState(state) {
    this.clusterState = state;
  }

  /**
   * Register an agent for monitoring
   * @param {AgentState} agentState
   */
  updateAgent(agentState) {
    this.agents.set(agentState.id, {
      ...agentState,
      lastUpdate: Date.now(),
    });
  }

  /**
   * Remove an agent from monitoring
   * @param {string} agentId
   */
  removeAgent(agentId) {
    this.agents.delete(agentId);
    this.metricsCache.delete(agentId);
  }

  /**
   * Get status icon for agent state
   * @param {string} state
   * @returns {string}
   */
  getAgentIcon(state) {
    switch (state) {
      case 'idle':
        return '⏳'; // Waiting for trigger
      case 'evaluating':
        return '🔍'; // Evaluating triggers
      case 'building_context':
        return '📝'; // Building context
      case 'executing':
        return '🔄'; // Running task
      case 'stopped':
        return '⏹️'; // Stopped
      case 'error':
        return '❌'; // Error
      default:
        return '⚪';
    }
  }

  /**
   * Format duration in human-readable form
   * @param {number} ms
   * @returns {string}
   */
  formatDuration(ms) {
    const seconds = Math.floor(ms / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);

    if (hours > 0) {
      return `${hours}h${minutes % 60}m`;
    }
    if (minutes > 0) {
      return `${minutes}m${seconds % 60}s`;
    }
    return `${seconds}s`;
  }

  /**
   * Render the footer
   */
  async render() {
    if (!this.enabled || !this.isTTY()) return;

    const { rows, cols } = this.getTerminalSize();

    // Collect metrics for all agents with PIDs
    for (const [agentId, agent] of this.agents) {
      if (agent.pid) {
        try {
          const metrics = await getProcessMetrics(agent.pid, { samplePeriodMs: 500 });
          this.metricsCache.set(agentId, metrics);
        } catch {
          // Process may have exited
          this.metricsCache.delete(agentId);
        }
      }
    }

    // Build footer content
    const line1 = this.buildAgentLine(cols);
    const line2 = this.buildStatusLine(cols);

    // Save cursor, render footer, restore cursor
    process.stdout.write(SAVE_CURSOR);
    process.stdout.write(HIDE_CURSOR);

    // Move to footer position
    this.moveTo(rows - 1, 1);
    process.stdout.write(CLEAR_LINE);
    process.stdout.write(`${COLORS.bgBlack}${line1}${COLORS.reset}`);

    this.moveTo(rows, 1);
    process.stdout.write(CLEAR_LINE);
    process.stdout.write(`${COLORS.bgBlack}${line2}${COLORS.reset}`);

    process.stdout.write(RESTORE_CURSOR);
    process.stdout.write(SHOW_CURSOR);
  }

  /**
   * Build the agent status line
   * @param {number} width - Terminal width
   * @returns {string}
   */
  buildAgentLine(width) {
    const parts = [];

    // Border
    parts.push(`${COLORS.gray}┌─${COLORS.reset}`);

    // Cluster ID
    if (this.clusterId) {
      const shortId = this.clusterId.replace('cluster-', '');
      parts.push(`${COLORS.cyan}${shortId}${COLORS.reset}`);
      parts.push(`${COLORS.gray} ─ ${COLORS.reset}`);
    }

    // Agent statuses
    const agentParts = [];
    for (const [agentId, agent] of this.agents) {
      const icon = this.getAgentIcon(agent.state);
      const metrics = this.metricsCache.get(agentId);

      let agentStr = `${icon} ${COLORS.white}${agentId}${COLORS.reset}`;

      if (metrics && metrics.exists) {
        const cpuColor = metrics.cpuPercent > 50 ? COLORS.yellow : COLORS.green;
        agentStr += ` ${cpuColor}${metrics.cpuPercent}%${COLORS.reset}`;
        agentStr += ` ${COLORS.gray}${metrics.memoryMB}MB${COLORS.reset}`;

        if (metrics.network.hasActivity) {
          agentStr += ` ${COLORS.cyan}↕${COLORS.reset}`;
        }
      }

      if (agent.iteration > 0) {
        agentStr += ` ${COLORS.dim}#${agent.iteration}${COLORS.reset}`;
      }

      agentParts.push(agentStr);
    }

    if (agentParts.length > 0) {
      parts.push(agentParts.join(`${COLORS.gray} │ ${COLORS.reset}`));
    } else {
      parts.push(`${COLORS.dim}No agents${COLORS.reset}`);
    }

    // Pad to width and close border
    const content = parts.join('');
    const contentLen = this.stripAnsi(content).length;
    const padding = Math.max(0, width - contentLen - 2);
    return content + ' '.repeat(padding) + `${COLORS.gray}─┐${COLORS.reset}`;
  }

  /**
   * Build the summary status line
   * @param {number} width - Terminal width
   * @returns {string}
   */
  buildStatusLine(width) {
    const parts = [];

    // Border
    parts.push(`${COLORS.gray}└─${COLORS.reset}`);

    // Cluster state
    const stateColor = this.clusterState === 'running' ? COLORS.green : COLORS.yellow;
    parts.push(`${stateColor}${this.clusterState}${COLORS.reset}`);

    // Duration
    const duration = this.formatDuration(Date.now() - this.startTime);
    parts.push(`${COLORS.gray} │ ${COLORS.reset}${COLORS.dim}${duration}${COLORS.reset}`);

    // Agent counts
    const executing = Array.from(this.agents.values()).filter(a => a.state === 'executing').length;
    const idle = Array.from(this.agents.values()).filter(a => a.state === 'idle').length;
    const total = this.agents.size;

    parts.push(`${COLORS.gray} │ ${COLORS.reset}`);
    parts.push(`${COLORS.green}${executing}/${total}${COLORS.reset} active`);

    // Aggregate metrics
    let totalCpu = 0;
    let totalMem = 0;
    for (const metrics of this.metricsCache.values()) {
      if (metrics.exists) {
        totalCpu += metrics.cpuPercent;
        totalMem += metrics.memoryMB;
      }
    }

    if (totalCpu > 0 || totalMem > 0) {
      parts.push(`${COLORS.gray} │ ${COLORS.reset}`);
      parts.push(`${COLORS.cyan}Σ${COLORS.reset} ${totalCpu.toFixed(0)}% ${totalMem.toFixed(0)}MB`);
    }

    // Pad and close
    const content = parts.join('');
    const contentLen = this.stripAnsi(content).length;
    const padding = Math.max(0, width - contentLen - 2);
    return content + ' '.repeat(padding) + `${COLORS.gray}─┘${COLORS.reset}`;
  }

  /**
   * Strip ANSI codes from string for length calculation
   * @param {string} str
   * @returns {string}
   */
  stripAnsi(str) {
    // eslint-disable-next-line no-control-regex
    return str.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, '');
  }

  /**
   * Start the status footer
   */
  start() {
    if (!this.enabled || !this.isTTY()) {
      return;
    }

    this.setupScrollRegion();

    // Handle terminal resize
    process.stdout.on('resize', () => {
      this.setupScrollRegion();
      this.render();
    });

    // Start refresh interval
    this.intervalId = setInterval(() => {
      this.render();
    }, this.refreshInterval);

    // Initial render
    this.render();
  }

  /**
   * Stop the status footer and cleanup
   */
  stop() {
    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }

    if (this.isTTY()) {
      // Reset scroll region
      this.resetScrollRegion();

      // Clear footer lines
      const { rows } = this.getTerminalSize();
      this.moveTo(rows - 1, 1);
      process.stdout.write(CLEAR_LINE);
      this.moveTo(rows, 1);
      process.stdout.write(CLEAR_LINE);

      // Move cursor to safe position
      this.moveTo(rows - 1, 1);
      process.stdout.write(SHOW_CURSOR);
    }
  }

  /**
   * Temporarily hide footer for clean output
   */
  hide() {
    if (!this.isTTY()) return;

    this.resetScrollRegion();
    const { rows } = this.getTerminalSize();
    this.moveTo(rows - 1, 1);
    process.stdout.write(CLEAR_LINE);
    this.moveTo(rows, 1);
    process.stdout.write(CLEAR_LINE);
  }

  /**
   * Restore footer after hiding
   */
  show() {
    if (!this.isTTY()) return;

    this.setupScrollRegion();
    this.render();
  }
}

/**
 * Create a singleton footer instance
 * @param {Object} options
 * @returns {StatusFooter}
 */
function createStatusFooter(options = {}) {
  return new StatusFooter(options);
}

module.exports = {
  StatusFooter,
  createStatusFooter,
};
