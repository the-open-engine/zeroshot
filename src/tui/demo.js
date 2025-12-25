/**
 * TUI Dashboard Demo
 * Simple demonstration of the dashboard layout with mock data
 *
 * Run: node src/tui/demo.js
 * Press: [q] to quit
 */

const blessed = require('blessed');
const {
  createLayout,
  updateClustersTable,
  updateAgentsTable,
  updateStatsBox,
  addLogEntry,
} = require('./layout');
const { formatTimestamp } = require('./formatters');

// Create main screen
const screen = blessed.screen({
  mouse: true,
  title: 'Cluster Dashboard - Demo',
  smartCSR: true,
});

// Create layout
const layout = createLayout(screen);

// Mock data generators
const mockClusters = [
  {
    id: 'cluster-swift-falcon',
    status: 'running',
    agentCount: 5,
    config: 'default',
    uptime: formatTimestamp(2 * 60 * 60 * 1000 + 30 * 60 * 1000), // 2h 30m
  },
  {
    id: 'cluster-bold-panther',
    status: 'running',
    agentCount: 3,
    config: 'simple',
    uptime: formatTimestamp(45 * 60 * 1000), // 45m
  },
  {
    id: 'cluster-quick-eagle',
    status: 'stopped',
    agentCount: 0,
    config: 'default',
    uptime: '0s',
  },
];

const mockAgents = [
  {
    clusterId: 'cluster-swift-falcon',
    id: 'worker-1',
    role: 'worker',
    status: 'running',
    iteration: 3,
    cpu: '12.5%',
    memory: '245 MB',
  },
  {
    clusterId: 'cluster-swift-falcon',
    id: 'validator-req',
    role: 'validator',
    status: 'idle',
    iteration: 0,
    cpu: '0.1%',
    memory: '128 MB',
  },
  {
    clusterId: 'cluster-swift-falcon',
    id: 'validator-sec',
    role: 'validator',
    status: 'idle',
    iteration: 0,
    cpu: '0.2%',
    memory: '135 MB',
  },
  {
    clusterId: 'cluster-bold-panther',
    id: 'worker-2',
    role: 'worker',
    status: 'running',
    iteration: 1,
    cpu: '8.3%',
    memory: '189 MB',
  },
  {
    clusterId: 'cluster-bold-panther',
    id: 'validator-qa',
    role: 'validator',
    status: 'running',
    iteration: 1,
    cpu: '5.1%',
    memory: '156 MB',
  },
];

const mockStats = {
  activeClusters: 2,
  totalAgents: 5,
  usedMemory: '853 MB',
  totalMemory: '8 GB',
  totalCPU: '26.2%',
};

// Keyboard shortcuts
screen.key(['q', 'C-c'], () => {
  return process.exit(0);
});

screen.key(['r'], () => {
  updateClustersTable(layout.clustersTable, mockClusters);
  updateAgentsTable(layout.agentTable, mockAgents);
  updateStatsBox(layout.statsBox, mockStats);
  addLogEntry(layout.logsBox, 'Dashboard refreshed', 'info');
  screen.render();
});

screen.key(['c'], () => {
  addLogEntry(layout.logsBox, 'Cluster started: cluster-wandering-wolf', 'info');
  screen.render();
});

screen.key(['k'], () => {
  addLogEntry(layout.logsBox, 'Cluster killed: cluster-quick-eagle', 'warn');
  screen.render();
});

screen.key(['s'], () => {
  addLogEntry(layout.logsBox, 'Warning: High memory usage on cluster-swift-falcon', 'warn');
  screen.render();
});

// Initialize with mock data
updateClustersTable(layout.clustersTable, mockClusters);
updateAgentsTable(layout.agentTable, mockAgents);
updateStatsBox(layout.statsBox, mockStats);

// Add initial log entries
addLogEntry(layout.logsBox, 'Dashboard initialized', 'info');
addLogEntry(layout.logsBox, 'Monitoring 2 active clusters', 'info');
addLogEntry(layout.logsBox, 'System CPU: 26.2% | Memory: 853 MB / 8 GB', 'info');

// Simulate live updates
const updateInterval = setInterval(() => {
  // Update uptime for running clusters
  mockClusters.forEach((cluster) => {
    if (cluster.status === 'running') {
      const uptimeMs = Math.random() * 3 * 60 * 60 * 1000; // Random uptime
      cluster.uptime = formatTimestamp(uptimeMs);
    }
  });

  // Simulate CPU/Memory changes
  mockAgents.forEach((agent) => {
    if (agent.status === 'running') {
      agent.cpu = (Math.random() * 20).toFixed(1) + '%';
      agent.memory = Math.floor(Math.random() * 200 + 100) + ' MB';
    }
  });

  mockStats.totalCPU = (Math.random() * 50).toFixed(1) + '%';

  updateClustersTable(layout.clustersTable, mockClusters);
  updateAgentsTable(layout.agentTable, mockAgents);
  updateStatsBox(layout.statsBox, mockStats);

  screen.render();
}, 3000);

// Display help on startup
setTimeout(() => {
  addLogEntry(
    layout.logsBox,
    'Press [r] to refresh | [c] to add cluster | [k] to kill | [s] for warning | [q] to quit',
    'info'
  );
  screen.render();
}, 500);

// Cleanup on exit
process.on('exit', () => {
  clearInterval(updateInterval);
});

// Render initial screen
screen.render();

console.log(
  '\n' +
    '===============================================\n' +
    '  Cluster Dashboard - Demo Mode\n' +
    '===============================================\n' +
    'Keyboard shortcuts:\n' +
    '  [↑/↓]     Navigate between widgets\n' +
    '  [Tab]     Next widget\n' +
    '  [Shift+Tab] Previous widget\n' +
    '  [r]       Refresh data\n' +
    '  [c]       Simulate cluster start\n' +
    '  [k]       Simulate cluster kill\n' +
    '  [s]       Simulate warning\n' +
    '  [q]       Quit\n' +
    '===============================================\n\n'
);
