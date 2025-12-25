/**
 * TUI - Main interactive dashboard
 *
 * Coordinates:
 * - Screen and layout
 * - Data polling
 * - Rendering
 * - Keybindings
 * - State management
 */

const blessed = require('blessed');
const { createLayout } = require('./layout');
const DataPoller = require('./data-poller');
const Renderer = require('./renderer');
const { setupKeybindings } = require('./keybindings');

class TUI {
  constructor(options) {
    this.orchestrator = options.orchestrator;
    this.filter = options.filter || 'running';
    this.refreshRate = options.refreshRate || 1000;

    // State
    this.clusters = [];
    this.resourceStats = new Map();
    this.messages = [];
    this.selectedIndex = 0;
    this.poller = null;
    this.renderer = null;
    this.widgets = null;
    this.screen = null;

    // View mode: 'overview' or 'detail'
    this.viewMode = 'overview';
    this.detailClusterId = null;
  }

  start() {
    // Create screen
    this.screen = blessed.screen({
      smartCSR: true,
      title: 'Vibe Cluster Watch',
      dockBorders: true,
      fullUnicode: true,
    });

    // Create layout
    this.widgets = createLayout(this.screen);

    // Show immediate loading message
    this.widgets.statsBox.setContent('{center}{bold}Loading...{/bold}{/center}');
    this.screen.render();

    // Create renderer
    this.renderer = new Renderer(this.widgets, this.screen);

    // Setup keybindings (pass TUI instance for state management)
    setupKeybindings(this.screen, this.widgets, this, this.orchestrator);

    // Create data poller
    this.poller = new DataPoller(this.orchestrator, {
      refreshRate: this.refreshRate,
      onUpdate: (update) => this._handleUpdate(update),
    });

    // Initial message
    this.messages.push({
      timestamp: new Date().toISOString(),
      text: 'TUI started. Press ? for help.',
      level: 'info',
    });
    this.renderer.renderLogs(this.messages.slice(-20));

    // Start polling
    this.poller.start();

    // Initial render
    this.screen.render();
  }

  _handleUpdate(update) {
    // Update state based on update.type
    switch (update.type) {
      case 'cluster_state':
        // Update cluster list
        this.clusters = update.clusters;

        // Apply filter
        let filteredClusters = this.clusters;
        if (this.filter === 'running') {
          // For "running" filter, only show truly active (running) clusters
          // Exclude initializing, stopped, failed, etc.
          filteredClusters = this.clusters.filter((c) => c.state === 'running');
        } else if (this.filter !== 'all') {
          // For other specific filters, match exact state
          filteredClusters = this.clusters.filter((c) => c.state === this.filter);
        }

        // Ensure selectedIndex is valid
        if (this.selectedIndex >= filteredClusters.length) {
          this.selectedIndex = Math.max(0, filteredClusters.length - 1);
        }

        // Render clusters table
        this.renderer.renderClustersTable(filteredClusters, this.selectedIndex);

        // Render system stats
        this.renderer.renderSystemStats(this.clusters, this.resourceStats);

        // Update agent table for selected cluster (ONLY in detail view)
        if (this.viewMode === 'detail' && this.detailClusterId) {
          // In detail view, show agents for the detail cluster
          try {
            const status = this.orchestrator.getStatus(this.detailClusterId);
            this.renderer.renderAgentTable(status.agents, this.resourceStats);
          } catch {
            // Cluster might have been stopped/killed
            this.renderer.renderAgentTable([], this.resourceStats);
          }
        } else if (this.viewMode === 'overview') {
          // In overview view, don't show agents (or show empty)
          this.renderer.renderAgentTable([], this.resourceStats);
        }
        break;

      case 'resource_stats':
        // Update resource stats
        this.resourceStats = update.stats;

        // Re-render system stats
        this.renderer.renderSystemStats(this.clusters, this.resourceStats);

        // Update agent table with new resource stats (ONLY in detail view)
        if (this.viewMode === 'detail' && this.detailClusterId) {
          try {
            const status = this.orchestrator.getStatus(this.detailClusterId);
            this.renderer.renderAgentTable(status.agents, this.resourceStats);
          } catch {
            this.renderer.renderAgentTable([], this.resourceStats);
          }
        }
        break;

      case 'new_message':
        // Only add messages from the selected cluster
        const selectedClusterId = this.renderer.selectedClusterId;
        if (selectedClusterId && update.clusterId === selectedClusterId) {
          // Add new message to log
          this.messages.push(update.message);

          // Keep only last 100 messages in memory
          if (this.messages.length > 100) {
            this.messages = this.messages.slice(-100);
          }

          // Render last 20 messages
          this.renderer.renderLogs(this.messages.slice(-20));
        }
        break;

      case 'error':
        // Add error to log
        this.messages.push({
          timestamp: new Date().toISOString(),
          text: `✗ ${update.error}`,
          level: 'error',
        });

        if (this.messages.length > 100) {
          this.messages = this.messages.slice(-100);
        }

        this.renderer.renderLogs(this.messages.slice(-20));
        break;
    }

    // Render screen
    this.screen.render();
  }

  exit() {
    if (this.poller) {
      this.poller.stop();
    }
    if (this.screen) {
      this.screen.destroy();
    }
    process.exit(0);
  }
}

module.exports = TUI;
