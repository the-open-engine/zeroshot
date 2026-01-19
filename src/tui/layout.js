/**
 * TUI Dashboard Layout
 * Creates a blessed-contrib grid with multiple widgets for cluster monitoring
 *
 * Layout Grid (20 rows x 12 columns):
 * - Rows 0-5:   Clusters Table (cols 0-7) | System Stats (cols 8-11)
 * - Rows 6-11:  Agents Table (cols 0-11)
 * - Rows 12-17: Live Logs (cols 0-11)
 * - Rows 18-19: Help Bar (cols 0-11)
 */

const blessed = require('blessed');
const contrib = require('blessed-contrib');

/**
 * Create main TUI layout with grid-based widget organization
 * @param {blessed.screen} screen - Blessed screen instance
 * @returns {object} Layout object containing all widgets
 */
function createLayout(screen) {
  // Create 20x12 grid for responsive layout
  const grid = new contrib.grid({ rows: 20, cols: 12, screen });

  // ============================================================
  // OVERVIEW MODE LAYOUT:
  // - Clusters Table (0-16 rows, 8 cols) - LARGE
  // - System Stats (0-6 rows, 4 cols)
  // - Help Bar (18-20 rows, 12 cols)
  //
  // DETAIL MODE LAYOUT:
  // - Agents Table (0-9 rows, 12 cols)
  // - Logs (9-18 rows, 12 cols)
  // - Help Bar (18-20 rows, 12 cols)
  // ============================================================

  const clustersTable = grid.set(0, 0, 15, 8, contrib.table, {
    keys: true,
    fg: 'white',
    selectedFg: 'black',
    selectedBg: 'cyan',
    interactive: true,
    label: ' Clusters ',
    border: { type: 'line', fg: 'cyan' },
    columnSpacing: 2,
    columnWidth: [15, 12, 8, 10, 8],
    style: {
      header: {
        fg: 'cyan',
        bold: true,
      },
      cell: {
        selected: {
          fg: 'black',
          bg: 'cyan',
        },
      },
    },
  });

  // Set initial columns for clusters table
  clustersTable.setData({
    headers: ['ID', 'Status', 'Agents', 'Config', 'Uptime'],
    data: [],
  });

  const statsBox = grid.set(0, 8, 15, 4, blessed.box, {
    label: ' System Stats ',
    content: '',
    tags: true,
    border: { type: 'line', fg: 'cyan' },
    style: {
      border: { fg: 'cyan' },
      label: { fg: 'cyan' },
    },
    padding: {
      left: 2,
      right: 2,
    },
  });

  // ============================================================
  // AGENTS TABLE (Detail mode only - 9 rows x 12 cols full width)
  // ============================================================

  const agentTable = grid.set(0, 0, 9, 12, contrib.table, {
    keys: true,
    fg: 'white',
    selectedFg: 'black',
    selectedBg: 'cyan',
    interactive: true,
    label: ' Agents ',
    border: { type: 'line', fg: 'cyan' },
    columnSpacing: 1,
    columnWidth: [12, 15, 12, 8, 8, 10, 10],
    style: {
      header: {
        fg: 'cyan',
        bold: true,
      },
      cell: {
        selected: {
          fg: 'black',
          bg: 'cyan',
        },
      },
    },
  });

  // Set initial columns for agents table
  agentTable.setData({
    headers: ['Cluster ID', 'Agent ID', 'Role', 'Status', 'Iter', 'CPU', 'Memory'],
    data: [],
  });

  // ============================================================
  // LOGS (Detail mode only - 9 rows x 12 cols full width)
  // ============================================================

  const logsBox = grid.set(9, 0, 9, 12, contrib.log, {
    fg: 'white',
    label: ' Live Logs ',
    border: { type: 'line', fg: 'cyan' },
    tags: true,
    style: {
      border: { fg: 'cyan' },
      label: { fg: 'cyan' },
      text: { fg: 'white' },
    },
    scrollable: true,
    mouse: true,
    keyable: true,
  });

  // ============================================================
  // WARNING BAR (experimental notice)
  // ============================================================

  const warningBar = grid.set(15, 0, 2, 12, blessed.box, {
    content: '{yellow-fg}⚠ Watch TUI is experimental{/}',
    tags: true,
    border: { type: 'line', fg: 'yellow' },
    style: {
      border: { fg: 'yellow' },
    },
    padding: {
      left: 1,
    },
  });

  // ============================================================
  // HELP BAR (2 rows x 12 cols):
  // - Keyboard shortcuts and commands
  // ============================================================

  const helpBar = grid.set(17, 0, 3, 12, blessed.box, {
    label: ' Help ',
    content:
      '{cyan-fg}[Enter]{/} View  ' +
      '{cyan-fg}[↑/↓]{/} Navigate  ' +
      '{cyan-fg}[K]{/} Kill  ' +
      '{cyan-fg}[s]{/} Stop  ' +
      '{cyan-fg}[l]{/} Logs  ' +
      '{cyan-fg}[r]{/} Refresh  ' +
      '{cyan-fg}[q]{/} Quit',
    tags: true,
    border: { type: 'line', fg: 'cyan' },
    style: {
      border: { fg: 'cyan' },
      label: { fg: 'cyan' },
      text: { fg: 'white' },
    },
    padding: {
      left: 1,
      right: 1,
    },
  });

  // Focus on clusters table by default
  clustersTable.focus();

  // Initially hide agent table and logs (overview mode)
  agentTable.hide();
  logsBox.hide();

  // ============================================================
  // Widget Navigation
  // ============================================================

  const widgets = [clustersTable, agentTable, logsBox];
  let currentFocus = 0;

  /**
   * Cycle focus to next widget (Tab key)
   */
  screen.key(['tab'], () => {
    currentFocus = (currentFocus + 1) % widgets.length;
    widgets[currentFocus].focus();
  });

  /**
   * Cycle focus to previous widget (Shift+Tab)
   */
  screen.key(['shift-tab'], () => {
    currentFocus = (currentFocus - 1 + widgets.length) % widgets.length;
    widgets[currentFocus].focus();
  });

  // ============================================================
  // Return all widgets for external access
  // ============================================================

  return {
    screen,
    grid,
    clustersTable,
    statsBox,
    agentTable,
    logsBox,
    warningBar,
    helpBar,
    widgets,
    focus: (widgetIndex) => {
      if (widgetIndex >= 0 && widgetIndex < widgets.length) {
        currentFocus = widgetIndex;
        widgets[widgetIndex].focus();
      }
    },
    getCurrentFocus: () => currentFocus,
  };
}

/**
 * Update clusters table with current cluster data
 * @param {object} clustersTable - Clusters table widget
 * @param {array} clusters - Array of cluster objects with properties: id, status, agentCount, config, uptime
 */
function updateClustersTable(clustersTable, clusters) {
  const data = clusters.map((cluster) => [
    cluster.id || 'N/A',
    cluster.status || 'unknown',
    String(cluster.agentCount || 0),
    cluster.config || 'N/A',
    cluster.uptime || '0s',
  ]);

  clustersTable.setData({
    headers: ['ID', 'Status', 'Agents', 'Config', 'Uptime'],
    data,
  });
}

/**
 * Update agents table with current agent data
 * @param {object} agentTable - Agents table widget
 * @param {array} agents - Array of agent objects
 */
function updateAgentsTable(agentTable, agents) {
  const data = agents.map((agent) => [
    agent.clusterId || 'N/A',
    agent.id || 'N/A',
    agent.role || 'worker',
    agent.status || 'idle',
    String(agent.iteration || 0),
    agent.cpu || '0.0%',
    agent.memory || '0 MB',
  ]);

  agentTable.setData({
    headers: ['Cluster ID', 'Agent ID', 'Role', 'Status', 'Iter', 'CPU', 'Memory'],
    data,
  });
}

/**
 * Update system stats box with current metrics
 * @param {object} statsBox - Stats box widget
 * @param {object} stats - Object with properties: totalMemory, usedMemory, totalCPU, activeClusters, totalAgents
 */
function updateStatsBox(statsBox, stats) {
  const content =
    `{cyan-fg}Active Clusters:{/}\n` +
    `  {white-fg}${stats.activeClusters || 0}{/}\n\n` +
    `{cyan-fg}Total Agents:{/}\n` +
    `  {white-fg}${stats.totalAgents || 0}{/}\n\n` +
    `{cyan-fg}System Memory:{/}\n` +
    `  {white-fg}${stats.usedMemory || '0 MB'}{/}\n` +
    `  {gray-fg}/ ${stats.totalMemory || '0 MB'}{/}\n\n` +
    `{cyan-fg}System CPU:{/}\n` +
    `  {white-fg}${stats.totalCPU || '0.0%'}{/}`;

  statsBox.setContent(content);
}

/**
 * Add log entry to logs widget
 * @param {object} logsBox - Logs box widget
 * @param {string} message - Log message
 * @param {string} level - Log level (info, warn, error, debug)
 */
function addLogEntry(logsBox, message, level = 'info') {
  const timestamp = new Date().toLocaleTimeString();
  const levelColor = {
    info: 'white-fg',
    warn: 'yellow-fg',
    error: 'red-fg',
    debug: 'gray-fg',
  };

  const color = levelColor[level] || 'white-fg';
  const logMessage = `{${color}}[${timestamp}]{/} ${message}`;

  logsBox.log(logMessage);
}

/**
 * Clear logs widget by resetting content
 * @param {object} logsBox - Logs box widget
 */
function clearLogs(logsBox) {
  // Log widget doesn't have clearData, so we destroy and recreate
  // Or use native content clearing
  if (logsBox._logLines) {
    logsBox._logLines = [];
  }
}

module.exports = {
  createLayout,
  updateClustersTable,
  updateAgentsTable,
  updateStatsBox,
  addLogEntry,
  clearLogs,
};
