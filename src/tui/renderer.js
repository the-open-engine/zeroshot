/**
 * TUI Screen Renderer
 * Transforms polled data into widget updates using formatters and layout widgets
 */

const { formatTimestamp, formatBytes, formatCPU, stateIcon, truncate } = require('./formatters');

class Renderer {
  /**
   * Create renderer instance
   * @param {object} widgets - Widget objects from layout.js
   * @param {object} screen - Blessed screen instance
   */
  constructor(widgets, screen) {
    if (!widgets) {
      throw new Error('Renderer requires widgets object from layout');
    }
    if (!screen) {
      throw new Error('Renderer requires screen instance');
    }

    this.widgets = widgets;
    this.screen = screen;
    this.selectedClusterId = null;
  }

  /**
   * Set the currently selected cluster ID
   * @param {string|null} id - Cluster ID to select
   */
  setSelectedCluster(id) {
    this.selectedClusterId = id;
  }

  /**
   * Render clusters table with state icons and uptime
   * @param {Array} clusters - Array of cluster objects
   */
  renderClustersTable(clusters) {
    if (!clusters || !Array.isArray(clusters)) {
      clusters = [];
    }

    const data = clusters.map((c) => {
      if (!c) return ['', '', '', ''];

      const icon = stateIcon(c.state || 'unknown');
      const uptime =
        c.state === 'running' && c.createdAt ? formatTimestamp(Date.now() - c.createdAt) : '-';
      const clusterId = truncate(c.id || '', 18);
      const state = (c.state || 'unknown').toUpperCase();
      const agentCount = `${c.agentCount || 0} agents`;

      return [`${icon} ${clusterId}`, state, agentCount, uptime];
    });

    if (this.widgets.clustersTable && this.widgets.clustersTable.setData) {
      this.widgets.clustersTable.setData({
        headers: ['ID', 'State', 'Agents', 'Uptime'],
        data,
      });
    }
  }

  /**
   * Render system statistics box with aggregate metrics
   * @param {Array} clusters - Array of cluster objects
   * @param {Map} resourceStats - Map of PID -> {cpu, memory}
   */
  renderSystemStats(clusters, resourceStats) {
    if (!clusters || !Array.isArray(clusters)) {
      clusters = [];
    }
    if (!resourceStats || !(resourceStats instanceof Map)) {
      resourceStats = new Map();
    }

    // Calculate aggregate stats
    const activeClusters = clusters.filter((c) => c && c.state === 'running').length;
    const totalAgents = clusters.reduce((sum, c) => sum + (c?.agentCount || 0), 0);

    // Calculate average CPU and memory from resource stats
    let totalCpu = 0;
    let totalMemory = 0;
    let statCount = 0;

    resourceStats.forEach((stat) => {
      if (stat && typeof stat.cpu === 'number' && typeof stat.memory === 'number') {
        totalCpu += stat.cpu;
        totalMemory += stat.memory;
        statCount++;
      }
    });

    const avgCpu = statCount > 0 ? totalCpu / statCount : 0;
    const avgMemory = statCount > 0 ? totalMemory / statCount : 0;

    // Format output with blessed color tags
    const statsText = [
      '{cyan-fg}Active Clusters:{/} ' + activeClusters,
      '{cyan-fg}Total Agents:{/}    ' + totalAgents,
      '{cyan-fg}Avg CPU:{/}         ' + formatCPU(avgCpu),
      '{cyan-fg}Avg Memory:{/}      ' + formatBytes(avgMemory),
    ].join('\n');

    if (this.widgets.statsBox && this.widgets.statsBox.setContent) {
      this.widgets.statsBox.setContent(statsText);
    }
  }

  /**
   * Render agent table for selected cluster
   * @param {Array} agents - Array of agent objects
   * @param {Map} resourceStats - Map of PID -> {cpu, memory}
   */
  renderAgentTable(agents, resourceStats) {
    if (!this.selectedClusterId) {
      // No cluster selected, show empty table
      if (this.widgets.agentTable && this.widgets.agentTable.setData) {
        this.widgets.agentTable.setData({
          headers: ['Agent', 'Role', 'State', 'Iter', 'CPU%', 'Mem'],
          data: [],
        });
      }
      return;
    }

    if (!agents || !Array.isArray(agents)) {
      agents = [];
    }
    if (!resourceStats || !(resourceStats instanceof Map)) {
      resourceStats = new Map();
    }

    const data = agents.map((a) => {
      if (!a) return ['', '', '', '', '', ''];

      const pid = a.pid;
      const stats = resourceStats.get(pid) || { cpu: 0, memory: 0 };

      const agentId = truncate(a.id || '', 12);
      const role = truncate(a.role || '', 12);
      const state = a.state || 'unknown';
      const iteration = `${a.iteration || 0}/${a.maxIterations || 0}`;
      const cpu = formatCPU(stats.cpu);
      const memory = formatBytes(stats.memory);

      return [agentId, role, state, iteration, cpu, memory];
    });

    if (this.widgets.agentTable && this.widgets.agentTable.setData) {
      this.widgets.agentTable.setData({
        headers: ['Agent', 'Role', 'State', 'Iter', 'CPU%', 'Mem'],
        data,
      });
    }
  }

  /**
   * Render log messages to log widget
   * @param {Array} messages - Array of message objects
   */
  renderLogs(messages) {
    if (!messages || !Array.isArray(messages)) {
      return;
    }

    if (!this.widgets.logsBox || !this.widgets.logsBox.log) {
      return;
    }

    messages.forEach((msg) => {
      if (!msg) return;

      const timestamp = msg.timestamp || Date.now();
      const time = new Date(timestamp).toLocaleTimeString();
      const sender = truncate(msg.sender || 'unknown', 15);
      const text = truncate(msg.content?.text || '', 60);

      this.widgets.logsBox.log(`[${time}] ${sender}: ${text}`);
    });
  }

  /**
   * Trigger screen render to update display
   */
  render() {
    if (this.screen && this.screen.render) {
      this.screen.render();
    }
  }
}

module.exports = Renderer;
