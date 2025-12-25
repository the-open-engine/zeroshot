# TUI Dashboard Layout Module

Dashboard layout builder for real-time cluster monitoring with blessed-contrib.

## Overview

The layout module creates a responsive terminal UI with a 20x12 grid layout containing:

- **Clusters Table** (top-left): View all active clusters, status, agent count, and uptime
- **System Stats** (top-right): CPU, memory, and cluster statistics
- **Agents Table** (middle): List all agents with role, status, iteration, and resource usage
- **Live Logs** (lower): Real-time event stream with color-coded severity levels
- **Help Bar** (bottom): Keyboard shortcut reference

## Grid Layout

```
┌─────────────────────────────────────────────┬─────────────────────┐
│ Clusters Table (6 rows x 8 cols)            │ System Stats Box    │
│                                             │ (6 rows x 4 cols)   │
├─────────────────────────────────────────────┴─────────────────────┤
│ Agents Table (6 rows x 12 cols)                                    │
├────────────────────────────────────────────────────────────────────┤
│ Live Logs (6 rows x 12 cols)                                       │
├────────────────────────────────────────────────────────────────────┤
│ Help Bar (2 rows x 12 cols)                                        │
└────────────────────────────────────────────────────────────────────┘
```

## Usage

### Basic Setup

```javascript
const blessed = require('blessed');
const { createLayout } = require('./layout');

// Create screen
const screen = blessed.screen({ mouse: true, title: 'Cluster Dashboard' });

// Create layout
const layout = createLayout(screen);

// Render
screen.render();

// Exit handler
screen.key(['q', 'C-c'], () => process.exit(0));
```

### Updating Tables

```javascript
const { updateClustersTable, updateAgentsTable, updateStatsBox, addLogEntry } = require('./layout');

// Update clusters table
updateClustersTable(layout.clustersTable, [
  {
    id: 'cluster-swift-falcon',
    status: 'running',
    agentCount: 5,
    config: 'default',
    uptime: '2h 30m',
  },
]);

// Update agents table
updateAgentsTable(layout.agentTable, [
  {
    clusterId: 'cluster-swift-falcon',
    id: 'worker-1',
    role: 'worker',
    status: 'running',
    iteration: 3,
    cpu: '12.5%',
    memory: '245 MB',
  },
]);

// Update system stats
updateStatsBox(layout.statsBox, {
  activeClusters: 2,
  totalAgents: 5,
  usedMemory: '512 MB',
  totalMemory: '8 GB',
  totalCPU: '26.2%',
});

// Add log entry
addLogEntry(layout.logsBox, 'Cluster started successfully', 'info');
addLogEntry(layout.logsBox, 'Warning: High CPU usage', 'warn');
addLogEntry(layout.logsBox, 'Error: Agent crashed', 'error');
```

## API Reference

### createLayout(screen)

Creates the dashboard layout with all widgets.

**Parameters:**

- `screen` (blessed.screen): Blessed screen instance

**Returns:**

```javascript
{
  (screen, // Blessed screen
    grid, // blessed-contrib grid
    clustersTable, // Clusters table widget
    agentTable, // Agents table widget
    statsBox, // System stats box widget
    logsBox, // Live logs widget
    helpBar, // Help bar widget
    widgets, // Array of interactive widgets [clustersTable, agentTable, logsBox]
    focus(index), // Function to focus widget by index
    getCurrentFocus()); // Function to get current focus index
}
```

### updateClustersTable(clustersTable, clusters)

Updates the clusters table with current data.

**Parameters:**

- `clustersTable`: Clusters table widget
- `clusters` (array): Array of cluster objects with properties:
  - `id` (string): Cluster identifier
  - `status` (string): running | stopped | initializing | stopping | failed | killed
  - `agentCount` (number): Number of agents
  - `config` (string): Configuration name
  - `uptime` (string): Formatted uptime (e.g., "2h 30m")

### updateAgentsTable(agentTable, agents)

Updates the agents table with current data.

**Parameters:**

- `agentTable`: Agents table widget
- `agents` (array): Array of agent objects with properties:
  - `clusterId` (string): Parent cluster ID
  - `id` (string): Agent identifier
  - `role` (string): worker | validator | orchestrator
  - `status` (string): running | idle | failed
  - `iteration` (number): Current iteration count
  - `cpu` (string): CPU percentage (e.g., "12.5%")
  - `memory` (string): Memory usage (e.g., "245 MB")

### updateStatsBox(statsBox, stats)

Updates the system stats box.

**Parameters:**

- `statsBox`: Stats box widget
- `stats` (object):
  - `activeClusters` (number): Count of active clusters
  - `totalAgents` (number): Total agent count
  - `usedMemory` (string): Formatted memory usage
  - `totalMemory` (string): Formatted total memory
  - `totalCPU` (string): Total CPU percentage

### addLogEntry(logsBox, message, level)

Adds a timestamped log entry.

**Parameters:**

- `logsBox`: Logs box widget
- `message` (string): Log message
- `level` (string): info | warn | error | debug (default: info)

### clearLogs(logsBox)

Clears all log entries.

**Parameters:**

- `logsBox`: Logs box widget

## Keyboard Navigation

| Key       | Action                     |
| --------- | -------------------------- |
| Tab       | Next widget                |
| Shift+Tab | Previous widget            |
| ↑/↓       | Navigate in focused widget |
| Enter     | Select/activate            |
| q         | Quit                       |

## Color Scheme

- **Borders**: Cyan
- **Headers**: Cyan (bold)
- **Text**: White
- **Selection**: Black text on cyan background
- **Log levels**:
  - info: White
  - warn: Yellow
  - error: Red
  - debug: Gray

## Demo

Run the included demo:

```bash
node src/tui/demo.js
```

Keyboard shortcuts in demo:

- [r] - Refresh data
- [c] - Simulate cluster start
- [k] - Simulate cluster kill
- [s] - Simulate warning
- [q] - Quit

## Testing

Run tests:

```bash
npm test -- tests/tui-layout.test.js
```

Tests verify:

- Layout creation and widget initialization
- Data update functions
- Focus navigation
- Log entry handling
- Edge cases (empty data, missing properties)

## Styling Customization

Widgets can be customized by modifying the configuration objects in `createLayout()`:

```javascript
const clustersTable = grid.set(0, 0, 6, 8, contrib.table, {
  fg: 'white', // Foreground color
  selectedFg: 'black', // Selected foreground
  selectedBg: 'cyan', // Selected background
  border: { type: 'line', fg: 'cyan' },
  style: {
    header: { fg: 'cyan', bold: true },
    cell: { selected: { fg: 'black', bg: 'cyan' } },
  },
});
```

## Related Modules

- `formatters.js` - Value formatting utilities (timestamps, bytes, CPU)
- `renderer.js` - Additional rendering helpers
- `keybindings.js` - Keyboard event handlers
- `data-poller.js` - Real-time data collection
- `index.js` - Main dashboard integration
