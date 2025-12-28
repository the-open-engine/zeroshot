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
 *
 * ROBUST RESIZE HANDLING:
 * - Debounced resize events (100ms) prevent rapid-fire redraws
 * - Render lock prevents concurrent renders from corrupting state
 * - Full footer clear before scroll region reset prevents artifacts
 * - Dimension checkpointing skips unnecessary redraws
 * - Graceful degradation for terminals < 8 rows
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
 * Debounce function - prevents rapid-fire calls during resize
 * @param {Function} fn - Function to debounce
 * @param {number} ms - Debounce delay in milliseconds
 * @returns {Function} Debounced function
 */
function debounce(fn, ms) {
  let timeout;
  return (...args) => {
    clearTimeout(timeout);
    timeout = setTimeout(() => fn(...args), ms);
  };
}

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
    this.messageBus = null; // MessageBus for token usage tracking

    // Robust resize handling state
    this.isRendering = false; // Render lock - prevents concurrent renders
    this.pendingResize = false; // Queue resize if render in progress
    this.lastKnownRows = 0; // Track terminal dimensions for change detection
    this.lastKnownCols = 0;
    this.minRows = 8; // Minimum rows for footer display (graceful degradation)
    this.hidden = false; // True when terminal too small for footer

    // Debounced resize handler (100ms) - prevents rapid-fire redraws
    this._debouncedResize = debounce(() => this._handleResize(), 100);
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
   * Clear a specific line completely
   * @param {number} row - 1-based row number
   * @private
   */
  _clearLine(row) {
    process.stdout.write(`${CSI}${row};1H${CLEAR_LINE}`);
  }

  /**
   * Generate move cursor ANSI sequence (returns string, doesn't write)
   * Used for atomic buffered writes to prevent interleaving
   * @param {number} row - 1-based row
   * @param {number} col - 1-based column
   * @returns {string} ANSI escape sequence
   * @private
   */
  _moveToStr(row, col) {
    return `${CSI}${row};${col}H`;
  }

  /**
   * Generate clear line ANSI sequence (returns string, doesn't write)
   * Used for atomic buffered writes to prevent interleaving
   * @param {number} row - 1-based row number
   * @returns {string} ANSI escape sequence
   * @private
   */
  _clearLineStr(row) {
    return `${CSI}${row};1H${CLEAR_LINE}`;
  }

  /**
   * Generate ANSI sequences to clear all footer lines (returns string)
   * Used for atomic buffered writes to prevent interleaving
   * @returns {string} ANSI escape sequences
   * @private
   */
  _clearFooterAreaStr() {
    const { rows } = this.getTerminalSize();
    // Use max of current and last footer height to ensure full cleanup
    const heightToClear = Math.max(this.footerHeight, this.lastFooterHeight, 3);
    const startRow = Math.max(1, rows - heightToClear + 1);

    let buffer = '';
    for (let row = startRow; row <= rows; row++) {
      buffer += this._clearLineStr(row);
    }
    return buffer;
  }

  /**
   * Clear all footer lines (uses last known height for safety)
   * Uses single atomic write to prevent interleaving with other processes
   * @private
   */
  _clearFooterArea() {
    process.stdout.write(this._clearFooterAreaStr());
  }

  /**
   * Set up scroll region to reserve space for footer
   * ROBUST: Clears footer area first, resets to full screen, then sets new region
   * Uses single atomic write to prevent interleaving with other processes
   */
  setupScrollRegion() {
    if (!this.isTTY()) return;

    const { rows, cols } = this.getTerminalSize();

    // Graceful degradation: hide footer if terminal too small
    if (rows < this.minRows) {
      if (!this.hidden) {
        this.hidden = true;
        // Reset to full screen scroll
        process.stdout.write(`${CSI}1;${rows}r`);
        this.scrollRegionSet = false;
      }
      return;
    }

    // Restore footer if terminal grew large enough
    if (this.hidden) {
      this.hidden = false;
    }

    const scrollEnd = rows - this.footerHeight;

    // BUILD ENTIRE OUTPUT INTO SINGLE BUFFER for atomic write
    let buffer = '';

    // Step 1: Save cursor before any manipulation
    buffer += SAVE_CURSOR;
    buffer += HIDE_CURSOR;

    // Step 2: Reset scroll region to full screen first (prevents artifacts)
    buffer += `${CSI}1;${rows}r`;

    // Step 3: Clear footer area completely (prevents ghosting)
    buffer += this._clearFooterAreaStr();

    // Step 4: Set new scroll region (lines 1 to scrollEnd)
    buffer += `${CSI}1;${scrollEnd}r`;

    // Step 5: Move cursor to bottom of scroll region (safe position)
    buffer += this._moveToStr(scrollEnd, 1);

    // Step 6: Restore cursor and show it
    buffer += RESTORE_CURSOR;
    buffer += SHOW_CURSOR;

    // SINGLE ATOMIC WRITE - prevents interleaving
    process.stdout.write(buffer);

    this.scrollRegionSet = true;
    this.lastKnownRows = rows;
    this.lastKnownCols = cols;
  }

  /**
   * Generate reset scroll region string (returns string, doesn't write)
   * @private
   */
  _resetScrollRegionStr() {
    const { rows } = this.getTerminalSize();
    return `${CSI}1;${rows}r`;
  }

  /**
   * Reset scroll region to full terminal
   */
  resetScrollRegion() {
    if (!this.isTTY()) return;

    process.stdout.write(this._resetScrollRegionStr());
    this.scrollRegionSet = false;
  }

  /**
   * Handle terminal resize event
   * Called via debounced wrapper to prevent rapid-fire redraws
   * @private
   */
  _handleResize() {
    if (!this.isTTY()) return;

    const { rows, cols } = this.getTerminalSize();

    // Skip if dimensions haven't actually changed (debounce may still fire)
    if (rows === this.lastKnownRows && cols === this.lastKnownCols) {
      return;
    }

    // If render in progress, queue resize for after
    if (this.isRendering) {
      this.pendingResize = true;
      return;
    }

    // Update dimensions and reconfigure
    this.lastKnownRows = rows;
    this.lastKnownCols = cols;

    this.setupScrollRegion();
    this.render();
  }

  /**
   * Register cluster for monitoring
   * @param {string} clusterId
   */
  setCluster(clusterId) {
    this.clusterId = clusterId;
  }

  /**
   * Set message bus for token usage tracking
   * @param {object} messageBus - MessageBus instance with getTokensByRole()
   */
  setMessageBus(messageBus) {
    this.messageBus = messageBus;
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
   * ROBUST: Uses render lock to prevent concurrent renders from corrupting state
   */
  async render() {
    if (!this.enabled || !this.isTTY()) return;

    // Graceful degradation: don't render if hidden
    if (this.hidden) return;

    // Render lock: prevent concurrent renders
    if (this.isRendering) {
      return;
    }
    this.isRendering = true;

    try {
      const { rows, cols } = this.getTerminalSize();

      // Double-check terminal size (may have changed since last check)
      if (rows < this.minRows) {
        this.hidden = true;
        this.resetScrollRegion();
        return;
      }

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

      // Calculate dynamic footer height: header + agent rows + summary
      // Minimum 3 lines (header + "no agents" message + summary)
      const agentRowCount = Math.max(1, executingAgents.length);
      const newHeight = 2 + agentRowCount + 1; // header + agents + summary

      // Update scroll region if height changed
      if (newHeight !== this.footerHeight) {
        this.lastFooterHeight = this.footerHeight;
        this.footerHeight = newHeight;
        this.setupScrollRegion();
      }

      // Build footer lines
      const headerLine = this.buildHeaderLine(cols);
      const agentRows = this.buildAgentRows(executingAgents, cols);
      const summaryLine = this.buildSummaryLine(cols);

      // BUILD ENTIRE OUTPUT INTO SINGLE BUFFER for atomic write
      // This prevents interleaving with other processes writing to stdout
      let buffer = '';
      buffer += SAVE_CURSOR;
      buffer += HIDE_CURSOR;

      // Render from top of footer area
      let currentRow = rows - this.footerHeight + 1;

      // Header line
      buffer += this._moveToStr(currentRow++, 1);
      buffer += CLEAR_LINE;
      buffer += `${COLORS.bgBlack}${headerLine}${COLORS.reset}`;

      // Agent rows
      for (const agentRow of agentRows) {
        buffer += this._moveToStr(currentRow++, 1);
        buffer += CLEAR_LINE;
        buffer += `${COLORS.bgBlack}${agentRow}${COLORS.reset}`;
      }

      // Summary line (with bottom border)
      buffer += this._moveToStr(currentRow, 1);
      buffer += CLEAR_LINE;
      buffer += `${COLORS.bgBlack}${summaryLine}${COLORS.reset}`;

      buffer += RESTORE_CURSOR;
      buffer += SHOW_CURSOR;

      // SINGLE ATOMIC WRITE - prevents interleaving
      process.stdout.write(buffer);
    } finally {
      this.isRendering = false;

      // Process pending resize if one was queued during render
      if (this.pendingResize) {
        this.pendingResize = false;
        // Use setImmediate to avoid deep recursion
        setImmediate(() => this._handleResize());
      }
    }
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
   * Build a single status line for testing/display
   * Alias for buildSummaryLine for backward compatibility
   * @param {number} width - Terminal width
   * @returns {string}
   */
  buildStatusLine(width) {
    return this.buildSummaryLine(width);
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
    const executing = Array.from(this.agents.values()).filter((a) => a.state === 'executing').length;
    const total = this.agents.size;
    parts.push(` ${COLORS.gray}│${COLORS.reset} ${COLORS.green}${executing}/${total}${COLORS.reset} active`);

    // Token cost (from message bus)
    if (this.messageBus && this.clusterId) {
      try {
        const tokensByRole = this.messageBus.getTokensByRole(this.clusterId);
        const totalCost = tokensByRole?._total?.totalCostUsd || 0;
        if (totalCost > 0) {
          // Format: $0.05 or $1.23 or $12.34
          const costStr = totalCost < 0.01 ? '<$0.01' : `$${totalCost.toFixed(2)}`;
          parts.push(` ${COLORS.gray}│${COLORS.reset} ${COLORS.yellow}${costStr}${COLORS.reset}`);
        }
      } catch {
        // Ignore errors - token tracking is optional
      }
    }

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

    // Initialize dimension tracking
    const { rows, cols } = this.getTerminalSize();
    this.lastKnownRows = rows;
    this.lastKnownCols = cols;

    // Check for graceful degradation at startup
    if (rows < this.minRows) {
      this.hidden = true;
      return; // Don't set up scroll region for tiny terminals
    }

    this.setupScrollRegion();

    // Handle terminal resize with debounced handler
    process.stdout.on('resize', this._debouncedResize);

    // Start refresh interval
    // Guard: Skip if previous render still running (prevents overlapping renders)
    this.intervalId = setInterval(() => {
      if (this.isRendering) return;
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

    // Remove resize listener
    process.stdout.removeListener('resize', this._debouncedResize);

    if (this.isTTY() && !this.hidden) {
      // BUILD SINGLE BUFFER for atomic shutdown write
      // Prevents interleaving with agent output during cleanup
      let buffer = '';

      // Reset scroll region
      buffer += this._resetScrollRegionStr();
      this.scrollRegionSet = false;

      // Clear all footer lines
      buffer += this._clearFooterAreaStr();

      // Move cursor to safe position and show cursor
      const { rows } = this.getTerminalSize();
      const startRow = rows - this.footerHeight + 1;
      buffer += this._moveToStr(startRow, 1);
      buffer += SHOW_CURSOR;

      // SINGLE ATOMIC WRITE
      process.stdout.write(buffer);
    }
  }

  /**
   * Temporarily hide footer for clean output
   */
  hide() {
    if (!this.isTTY()) return;

    // Single atomic write for hide operation
    let buffer = this._resetScrollRegionStr();
    this.scrollRegionSet = false;
    buffer += this._clearFooterAreaStr();
    process.stdout.write(buffer);
  }

  /**
   * Restore footer after hiding
   */
  show() {
    if (!this.isTTY()) return;

    // Reset hidden state and check terminal size
    const { rows } = this.getTerminalSize();
    if (rows < this.minRows) {
      this.hidden = true;
      return;
    }

    this.hidden = false;
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
