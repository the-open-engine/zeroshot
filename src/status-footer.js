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

const { getProcessMetrics } = require('./process-metrics');

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
   * @param {number} [options.maxAgentRows=5] - Max agent rows to display
   */
  constructor(options = {}) {
    this.refreshInterval = options.refreshInterval || 1000;
    this.enabled = options.enabled !== false;
    this.maxAgentRows = options.maxAgentRows || 5;
    this.intervalId = null;
    this.agents = new Map(); // agentId -> AgentState
    this.metricsCache = new Map(); // agentId -> ProcessMetrics
    this.footerHeight = 3; // Minimum: header + 1 agent row + summary
    this.lastFooterHeight = 3;
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
   * Format bytes in human-readable form
   * @param {number} bytes
   * @returns {string}
   */
  formatBytes(bytes) {
    if (bytes < 1024) {
      return `${bytes}B`;
    }
    if (bytes < 1024 * 1024) {
      return `${(bytes / 1024).toFixed(1)}KB`;
    }
    if (bytes < 1024 * 1024 * 1024) {
      return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
    }
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)}GB`;
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

    // Get executing agents for display
    const executingAgents = Array.from(this.agents.entries())
      .filter(([, agent]) => agent.state === 'executing')
      .slice(0, this.maxAgentRows);

    // Calculate dynamic footer height: header + agent rows + separator + summary
    // Minimum 3 lines (header + "no agents" message + summary)
    const agentRowCount = Math.max(1, executingAgents.length);
    const newHeight = 2 + agentRowCount + 1; // header + agents + summary (separator merged with summary)

    // Update scroll region if height changed
    if (newHeight !== this.footerHeight) {
      this.footerHeight = newHeight;
      this.setupScrollRegion();
    }

    // Build footer lines
    const headerLine = this.buildHeaderLine(cols);
    const agentRows = this.buildAgentRows(executingAgents, cols);
    const summaryLine = this.buildSummaryLine(cols);

    // Save cursor, render footer, restore cursor
    process.stdout.write(SAVE_CURSOR);
    process.stdout.write(HIDE_CURSOR);

    // Render from top of footer area
    let currentRow = rows - this.footerHeight + 1;

    // Header line
    this.moveTo(currentRow++, 1);
    process.stdout.write(CLEAR_LINE);
    process.stdout.write(`${COLORS.bgBlack}${headerLine}${COLORS.reset}`);

    // Agent rows
    for (const agentRow of agentRows) {
      this.moveTo(currentRow++, 1);
      process.stdout.write(CLEAR_LINE);
      process.stdout.write(`${COLORS.bgBlack}${agentRow}${COLORS.reset}`);
    }

    // Summary line (with bottom border)
    this.moveTo(currentRow, 1);
    process.stdout.write(CLEAR_LINE);
    process.stdout.write(`${COLORS.bgBlack}${summaryLine}${COLORS.reset}`);

    process.stdout.write(RESTORE_CURSOR);
    process.stdout.write(SHOW_CURSOR);
  }

  /**
   * Build the header line with cluster ID
   * @param {number} width - Terminal width
   * @returns {string}
   */
  buildHeaderLine(width) {
    let content = `${COLORS.gray}┌─${COLORS.reset}`;

    // Cluster ID
    if (this.clusterId) {
      const shortId = this.clusterId.replace('cluster-', '');
      content += ` ${COLORS.cyan}${COLORS.bold}${shortId}${COLORS.reset} `;
    }

    // Fill with border
    const contentLen = this.stripAnsi(content).length;
    const padding = Math.max(0, width - contentLen - 1);
    return content + `${COLORS.gray}${'─'.repeat(padding)}┐${COLORS.reset}`;
  }

  /**
   * Build agent rows (one row per executing agent)
   * @param {Array} executingAgents - Array of [agentId, agent] pairs
   * @param {number} width - Terminal width
   * @returns {Array<string>} Array of formatted rows
   */
  buildAgentRows(executingAgents, width) {
    if (executingAgents.length === 0) {
      // No agents row
      const content = `${COLORS.gray}│${COLORS.reset}  ${COLORS.dim}No active agents${COLORS.reset}`;
      const contentLen = this.stripAnsi(content).length;
      const padding = Math.max(0, width - contentLen - 1);
      return [content + ' '.repeat(padding) + `${COLORS.gray}│${COLORS.reset}`];
    }

    const rows = [];
    for (const [agentId, agent] of executingAgents) {
      const icon = this.getAgentIcon(agent.state);
      const metrics = this.metricsCache.get(agentId);

      // Build columns with fixed widths for alignment
      const iconCol = icon;
      const nameCol = agentId.padEnd(14).slice(0, 14); // Max 14 chars for name

      let metricsStr = '';
      if (metrics && metrics.exists) {
        const cpuColor = metrics.cpuPercent > 50 ? COLORS.yellow : COLORS.green;
        const cpuVal = `${metrics.cpuPercent}%`.padStart(4);
        const ramVal = `${metrics.memoryMB}MB`.padStart(6);

        metricsStr += `${COLORS.dim}CPU:${COLORS.reset}${cpuColor}${cpuVal}${COLORS.reset}`;
        metricsStr += `  ${COLORS.dim}RAM:${COLORS.reset}${COLORS.gray}${ramVal}${COLORS.reset}`;

        // Network bytes
        const net = metrics.network;
        if (net.bytesSent > 0 || net.bytesReceived > 0) {
          const sent = this.formatBytes(net.bytesSent).padStart(7);
          const recv = this.formatBytes(net.bytesReceived).padStart(7);
          metricsStr += `  ${COLORS.dim}NET:${COLORS.reset}${COLORS.cyan}↑${sent} ↓${recv}${COLORS.reset}`;
        }
      } else {
        metricsStr = `${COLORS.dim}(starting...)${COLORS.reset}`;
      }

      // Iteration number
      const iterStr = agent.iteration > 0 ? `${COLORS.dim}#${agent.iteration}${COLORS.reset}` : '';

      // Build the row
      let content = `${COLORS.gray}│${COLORS.reset}  ${iconCol} ${COLORS.white}${nameCol}${COLORS.reset}  ${metricsStr}`;
      if (iterStr) {
        content += `  ${iterStr}`;
      }

      const contentLen = this.stripAnsi(content).length;
      const padding = Math.max(0, width - contentLen - 1);
      rows.push(content + ' '.repeat(padding) + `${COLORS.gray}│${COLORS.reset}`);
    }

    return rows;
  }

  /**
   * Build the summary line with aggregated metrics
   * @param {number} width - Terminal width
   * @returns {string}
   */
  buildSummaryLine(width) {
    const parts = [];

    // Border with corner
    parts.push(`${COLORS.gray}└─${COLORS.reset}`);

    // Cluster state
    const stateColor = this.clusterState === 'running' ? COLORS.green : COLORS.yellow;
    parts.push(` ${stateColor}${this.clusterState}${COLORS.reset}`);

    // Duration
    const duration = this.formatDuration(Date.now() - this.startTime);
    parts.push(` ${COLORS.gray}│${COLORS.reset} ${COLORS.dim}${duration}${COLORS.reset}`);

    // Agent counts
    const executing = Array.from(this.agents.values()).filter(a => a.state === 'executing').length;
    const total = this.agents.size;
    parts.push(` ${COLORS.gray}│${COLORS.reset} ${COLORS.green}${executing}/${total}${COLORS.reset} active`);

    // Aggregate metrics
    let totalCpu = 0;
    let totalMem = 0;
    let totalBytesSent = 0;
    let totalBytesReceived = 0;
    for (const metrics of this.metricsCache.values()) {
      if (metrics.exists) {
        totalCpu += metrics.cpuPercent;
        totalMem += metrics.memoryMB;
        totalBytesSent += metrics.network.bytesSent || 0;
        totalBytesReceived += metrics.network.bytesReceived || 0;
      }
    }

    if (totalCpu > 0 || totalMem > 0) {
      parts.push(` ${COLORS.gray}│${COLORS.reset}`);
      let aggregateStr = ` ${COLORS.cyan}Σ${COLORS.reset} `;
      aggregateStr += `${COLORS.dim}CPU:${COLORS.reset}${totalCpu.toFixed(0)}%`;
      aggregateStr += ` ${COLORS.dim}RAM:${COLORS.reset}${totalMem.toFixed(0)}MB`;
      if (totalBytesSent > 0 || totalBytesReceived > 0) {
        aggregateStr += ` ${COLORS.dim}NET:${COLORS.reset}${COLORS.cyan}↑${this.formatBytes(totalBytesSent)} ↓${this.formatBytes(totalBytesReceived)}${COLORS.reset}`;
      }
      parts.push(aggregateStr);
    }

    // Pad and close with bottom corner
    const content = parts.join('');
    const contentLen = this.stripAnsi(content).length;
    const padding = Math.max(0, width - contentLen - 1);
    return content + `${COLORS.gray}${'─'.repeat(padding)}┘${COLORS.reset}`;
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

      // Clear all footer lines (dynamic height)
      const { rows } = this.getTerminalSize();
      const startRow = rows - this.footerHeight + 1;
      for (let row = startRow; row <= rows; row++) {
        this.moveTo(row, 1);
        process.stdout.write(CLEAR_LINE);
      }

      // Move cursor to safe position
      this.moveTo(startRow, 1);
      process.stdout.write(SHOW_CURSOR);
    }
  }

  /**
   * Temporarily hide footer for clean output
   */
  hide() {
    if (!this.isTTY()) return;

    this.resetScrollRegion();

    // Clear all footer lines (dynamic height)
    const { rows } = this.getTerminalSize();
    const startRow = rows - this.footerHeight + 1;
    for (let row = startRow; row <= rows; row++) {
      this.moveTo(row, 1);
      process.stdout.write(CLEAR_LINE);
    }
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
