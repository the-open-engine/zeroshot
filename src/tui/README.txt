VIBE WATCH - Interactive TUI Dashboard
=======================================

Launch with: vibe watch

OVERVIEW
--------
The vibe watch command provides a real-time, htop/k9s-style dashboard for monitoring all active vibe clusters.

FEATURES
--------
✓ Real-time cluster state monitoring (1s refresh)
✓ CPU and memory tracking per agent (via pidusage)
✓ Live message streaming from cluster ledgers
✓ Interactive keyboard controls (kill, stop, export)
✓ Automatic detection of new clusters
✓ System-wide statistics (active clusters, agents, avg resources)

LAYOUT
------
┌─────────────────────────────────────────────────────────┐
│ VIBE CLUSTER WATCH                        [q] Quit      │
├─────────────────────────────────────────────────────────┤
│ ┌─ Clusters ─────────┐ ┌─ System Stats ──────────────┐ │
│ │ ID     State  Time  │ │ Active: 2   CPU: 12%       │ │
│ │ ● a-38 RUN    5m    │ │ Agents: 7   Mem: 245 MB    │ │
│ │ ● s-62 RUN    2m    │ └────────────────────────────┘ │
│ └────────────────────┘                                  │
│ ┌─ Agents ───────────────────────────────────────────┐ │
│ │ Agent    Role     State    Iter  CPU%  Mem(MB)    │ │
│ │ worker   impl     exec     3     8.5   67         │ │
│ │ validator val     idle     1     0.1   42         │ │
│ └────────────────────────────────────────────────────┘ │
│ ┌─ Live Logs ────────────────────────────────────────┐ │
│ │ [09:45:23] worker: TASK_STARTED (iteration 3)     │ │
│ │ [09:45:24] worker: Implementing feature X...      │ │
│ └────────────────────────────────────────────────────┘ │
│ ┌─ Help ─────────────────────────────────────────────┐ │
│ │ [↑/↓] Nav  [K] Kill  [s] Stop  [e] Export  [q] Quit│ │
│ └────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘

KEYBOARD SHORTCUTS
------------------
Navigation:
  ↑ / k          Move selection up
  ↓ / j          Move selection down

Actions (on selected cluster):
  K              Kill cluster (force, with confirmation)
  s              Stop cluster (graceful, with confirmation)
  e              Export cluster conversation (markdown)
  l              Open full logs in new terminal window

View Controls:
  r              Force refresh all data
  f              Toggle filter (running/stopped/all)
  ? / h          Show help dialog

Exit:
  q / Ctrl-C     Quit (with confirmation if clusters active)

USAGE
-----
# Launch with defaults
vibe watch

# Filter to only running clusters
vibe watch --filter running

# Faster refresh (500ms instead of 1s)
vibe watch --refresh-rate 500

# Show only stopped clusters
vibe watch --filter stopped

ARCHITECTURE
------------
The TUI is composed of 6 modular components:

1. index.js       - Main coordinator, initializes screen/layout/poller
2. layout.js      - Creates blessed-contrib grid with 5 widgets
3. renderer.js    - Transforms data into widget updates
4. data-poller.js - Polls orchestrator at 4 different intervals
5. keybindings.js - Keyboard event handlers and confirmations
6. formatters.js  - Utility functions (time, bytes, CPU, icons)

Data Flow:
  Orchestrator → DataPoller → TUI.onUpdate → Renderer → Widgets → Screen

Polling Strategy:
  - Cluster states: 1s (main refresh rate)
  - Resource stats: 2s (expensive pidusage calls)
  - New clusters:   2s (rare event)
  - Log messages:   500ms per cluster (real-time feel)

DEPENDENCIES
------------
- blessed@0.1.81          Terminal UI framework
- blessed-contrib@4.11.0  Dashboard widgets (grid, table, log)
- pidusage@4.0.1          Cross-platform CPU/memory monitoring

TESTING
-------
# Run integration test
node tests/tui-integration.test.js

# Run layout demo (interactive)
node src/tui/demo.js

# Run unit tests
npm test

TROUBLESHOOTING
---------------
Q: TUI shows "No clusters found"
A: Start a cluster first: vibe run "test task" or vibe task run "test"

Q: CPU/Memory shows 0%
A: Process may have died. Check cluster state.

Q: Logs not streaming
A: Ledger database may be locked. Wait a few seconds.

Q: Terminal garbled on exit
A: Try running: reset

Q: Keyboard shortcuts not working
A: Make sure terminal supports key events (most modern terminals do)

DEMO MODE
---------
To see the TUI with mock data:
  node src/tui/demo.js

This starts a live dashboard with simulated clusters that auto-updates.
Press [r] to refresh, [c] to add cluster, [k] to kill, [q] to quit.

FILES
-----
src/tui/
├── index.js          Main TUI class (6.0K)
├── layout.js         Widget creation (8.1K)
├── renderer.js       Data → widgets (5.6K)
├── data-poller.js    Data collection (8.1K)
├── keybindings.js    User input (8.9K)
├── formatters.js     Utilities (3.4K)
├── demo.js           Interactive demo (5.0K)
└── LAYOUT.md         API documentation (7.4K)

tests/
└── tui-integration.test.js  Integration test

MODIFICATIONS TO EXISTING FILES
-------------------------------
src/agent-wrapper.js
  - Added: this.processPid tracking (line 42)
  - Added: PID capture on spawn (line 605-607)
  - Added: PROCESS_SPAWNED lifecycle event
  - Added: TASK_ID_ASSIGNED lifecycle event
  - Added: pid field in getState() (line 1350)

cli/index.js
  - Added: vibe watch command with options

lib/completion.js
  - Added: Shell completion for watch command

package.json
  - Added: blessed, blessed-contrib, pidusage dependencies

FUTURE ENHANCEMENTS
-------------------
Potential improvements:
- [ ] Sorting clusters by various fields
- [ ] Filtering by cluster config name
- [ ] Graph view for CPU/memory over time
- [ ] Search/filter logs by keyword
- [ ] Export selected logs to file
- [ ] Cluster health indicators
- [ ] Alert notifications for failures
- [ ] Docker container stats (for isolation mode)
- [ ] Network I/O stats
- [ ] Agent communication graph visualization

CREDITS
-------
Implementation: 6 parallel agents (Dec 2024)
Architecture: blessed-contrib grid system
Inspiration: htop, k9s, lazydocker

For issues or feature requests, see vibe/cluster GitHub repo.
