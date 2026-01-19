/**
 * Test suite for TUI layout
 * Verifies layout creation and widget updates work correctly
 */

const { expect } = require('chai');
const blessed = require('blessed');
const {
  createLayout,
  updateClustersTable,
  updateAgentsTable,
  updateStatsBox,
  addLogEntry,
  clearLogs,
} = require('../src/tui/layout');

let screen;
let layout;

describe('TUI Layout', () => {
  beforeEach(() => {
    // Create a mock screen for testing
    screen = blessed.screen({ mouse: true, title: 'Test Dashboard' });
  });

  afterEach(() => {
    if (screen) {
      screen.destroy();
    }
  });

  defineCreateLayoutTests();
  defineUpdateClustersTableTests();
  defineUpdateAgentsTableTests();
  defineUpdateStatsBoxTests();
  defineAddLogEntryTests();
  defineClearLogsTests();
  defineFocusNavigationTests();
});

function defineCreateLayoutTests() {
  describe('createLayout', () => {
    it('should create layout with all widgets', () => {
      layout = createLayout(screen);

      expect(layout).to.exist;
      expect(layout.screen).to.equal(screen);
      expect(layout.grid).to.exist;
      expect(layout.clustersTable).to.exist;
      expect(layout.agentTable).to.exist;
      expect(layout.statsBox).to.exist;
      expect(layout.logsBox).to.exist;
      expect(layout.helpBar).to.exist;
    });

    it('should have widgets array with 3 items', () => {
      layout = createLayout(screen);

      expect(layout.widgets).to.be.an('array');
      expect(layout.widgets).to.have.lengthOf(3);
    });

    it('should initialize clusters table with empty data', () => {
      layout = createLayout(screen);

      // Table should have headers and empty data initially
      expect(layout.clustersTable).to.exist;
    });

    it('should provide focus control methods', () => {
      layout = createLayout(screen);

      expect(layout.focus).to.be.a('function');
      expect(layout.getCurrentFocus).to.be.a('function');
    });

    it('should set focus to clusters table initially', () => {
      layout = createLayout(screen);

      expect(layout.getCurrentFocus()).to.equal(0);
    });
  });
}

function defineUpdateClustersTableTests() {
  describe('updateClustersTable', () => {
    beforeEach(() => {
      layout = createLayout(screen);
    });

    it('should update table with cluster data', () => {
      const clusters = [
        {
          id: 'cluster-1',
          status: 'running',
          agentCount: 5,
          config: 'default',
          uptime: '2h 30m',
        },
        {
          id: 'cluster-2',
          status: 'stopped',
          agentCount: 0,
          config: 'simple',
          uptime: '0s',
        },
      ];

      updateClustersTable(layout.clustersTable, clusters);

      // Verify the method completes without error
      expect(layout.clustersTable).to.exist;
    });

    it('should handle empty cluster array', () => {
      updateClustersTable(layout.clustersTable, []);

      expect(layout.clustersTable).to.exist;
    });

    it('should handle clusters with missing properties', () => {
      const clusters = [
        {
          id: 'cluster-1',
          // missing other properties
        },
      ];

      updateClustersTable(layout.clustersTable, clusters);

      expect(layout.clustersTable).to.exist;
    });
  });
}

function defineUpdateAgentsTableTests() {
  describe('updateAgentsTable', () => {
    beforeEach(() => {
      layout = createLayout(screen);
    });

    it('should update table with agent data', () => {
      const agents = [
        {
          clusterId: 'cluster-1',
          id: 'worker-1',
          role: 'worker',
          status: 'running',
          iteration: 3,
          cpu: '12.5%',
          memory: '245 MB',
        },
        {
          clusterId: 'cluster-1',
          id: 'validator-1',
          role: 'validator',
          status: 'idle',
          iteration: 0,
          cpu: '0.1%',
          memory: '128 MB',
        },
      ];

      updateAgentsTable(layout.agentTable, agents);

      expect(layout.agentTable).to.exist;
    });

    it('should handle empty agent array', () => {
      updateAgentsTable(layout.agentTable, []);

      expect(layout.agentTable).to.exist;
    });

    it('should handle agents with missing properties', () => {
      const agents = [
        {
          id: 'agent-1',
          // missing other properties
        },
      ];

      updateAgentsTable(layout.agentTable, agents);

      expect(layout.agentTable).to.exist;
    });
  });
}

function defineUpdateStatsBoxTests() {
  describe('updateStatsBox', () => {
    beforeEach(() => {
      layout = createLayout(screen);
    });

    it('should update stats box with system metrics', () => {
      const stats = {
        activeClusters: 3,
        totalAgents: 15,
        usedMemory: '512 MB',
        totalMemory: '2 GB',
        totalCPU: '25.5%',
      };

      updateStatsBox(layout.statsBox, stats);

      expect(layout.statsBox).to.exist;
    });

    it('should handle missing stats properties', () => {
      const stats = {
        activeClusters: 2,
        // missing other properties
      };

      updateStatsBox(layout.statsBox, stats);

      expect(layout.statsBox).to.exist;
    });

    it('should handle empty stats object', () => {
      updateStatsBox(layout.statsBox, {});

      expect(layout.statsBox).to.exist;
    });
  });
}

function defineAddLogEntryTests() {
  describe('addLogEntry', () => {
    beforeEach(() => {
      layout = createLayout(screen);
    });

    it('should add info level log entry', () => {
      addLogEntry(layout.logsBox, 'Test info message', 'info');

      expect(layout.logsBox).to.exist;
    });

    it('should add warn level log entry', () => {
      addLogEntry(layout.logsBox, 'Test warning message', 'warn');

      expect(layout.logsBox).to.exist;
    });

    it('should add error level log entry', () => {
      addLogEntry(layout.logsBox, 'Test error message', 'error');

      expect(layout.logsBox).to.exist;
    });

    it('should add debug level log entry', () => {
      addLogEntry(layout.logsBox, 'Test debug message', 'debug');

      expect(layout.logsBox).to.exist;
    });

    it('should default to info level if not specified', () => {
      addLogEntry(layout.logsBox, 'Test message without level');

      expect(layout.logsBox).to.exist;
    });

    it('should handle unknown log level', () => {
      addLogEntry(layout.logsBox, 'Test unknown level', 'unknown');

      expect(layout.logsBox).to.exist;
    });
  });
}

function defineClearLogsTests() {
  describe('clearLogs', () => {
    beforeEach(() => {
      layout = createLayout(screen);
    });

    it('should clear logs without error', () => {
      addLogEntry(layout.logsBox, 'Test message 1');
      addLogEntry(layout.logsBox, 'Test message 2');

      clearLogs(layout.logsBox);

      expect(layout.logsBox).to.exist;
    });
  });
}

function defineFocusNavigationTests() {
  describe('focus navigation', () => {
    beforeEach(() => {
      layout = createLayout(screen);
    });

    it('should cycle focus through widgets', () => {
      expect(layout.getCurrentFocus()).to.equal(0); // clusters table

      layout.focus(1);
      expect(layout.getCurrentFocus()).to.equal(1); // agents table

      layout.focus(2);
      expect(layout.getCurrentFocus()).to.equal(2); // logs

      layout.focus(0);
      expect(layout.getCurrentFocus()).to.equal(0); // back to clusters
    });

    it('should not focus on invalid indices', () => {
      const initialFocus = layout.getCurrentFocus();

      layout.focus(-1);
      expect(layout.getCurrentFocus()).to.equal(initialFocus);

      layout.focus(999);
      expect(layout.getCurrentFocus()).to.equal(initialFocus);
    });
  });
}
