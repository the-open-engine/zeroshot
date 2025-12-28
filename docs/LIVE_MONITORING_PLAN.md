# Live Monitoring for Zeroshot

## Problem Statement

When running `zeroshot run` or `zeroshot task`, users can only see agent output. They have no visibility into:
- Which workers are active vs idle
- CPU/memory usage per worker
- Network activity (API calls in progress)
- Whether a worker is stuck or making progress

## Design Goals

1. **Always visible** - Status footer shown during ALL zeroshot executions (not just attach)
2. **Non-intrusive** - Footer doesn't disrupt terminal output scrolling
3. **Real-time** - Update every 1-2 seconds
4. **Low overhead** - Minimal CPU cost for monitoring itself
5. **Cross-platform** - Linux-first (/proc), graceful degradation on macOS

## Architecture Options

### Option A: Status Bar in AttachClient (Recommended for MVP)

Add a persistent header showing metrics for the attached agent:

```
┌─ worker [sonnet] ─ CPU: 12% │ Mem: 45MB │ Net: ↓2.1KB/s ↑0.3KB/s │ Tokens: 1.2K ─┐
│ ... terminal output ...                                                           │
└───────────────────────────────────────────────────────────────────────────────────┘
```

**Pros:**
- Simple implementation
- Natural extension of attach
- Single agent focus

**Cons:**
- Only one agent visible
- Requires terminal manipulation

### Option B: Enhance `zeroshot watch` (Recommended for Full Solution)

Extend existing TUI dashboard with per-agent metrics:

```
┌─ Cluster: cosmic-meteor-87 ───────────────────────────────────────────────────────┐
│                                                                                    │
│  AGENT         STATE      CPU    MEM     NET I/O       TOKENS    LAST OUTPUT      │
│  ─────────────────────────────────────────────────────────────────────────────    │
│  worker        executing  23%    67MB    ↓4.2K ↑1.1K   3.4K      2s ago           │
│  validator-1   idle       0%     12MB    -             0         waiting          │
│  validator-2   idle       0%     12MB    -             0         waiting          │
│                                                                                    │
│  [CPU CHART]                           [NETWORK CHART]                            │
│  ███████░░░░░░░░░░░░ 23%               ▁▂▃▅▇█▅▃▂▁ 4.2KB/s                        │
│                                                                                    │
└─ Press 'q' to quit │ 'a' to attach │ 'l' for logs ─────────────────────────────────┘
```

**Pros:**
- All agents visible
- Charts for trends
- Interactive (attach from dashboard)

**Cons:**
- More complex
- Can't see terminal output simultaneously

### Option C: Side-by-Side Split (Future)

Terminal multiplexing with tmux-style panes.

## Implementation Phases

### Phase 1: Process Metrics Module (1-2 hours)

Create `src/process-metrics.js`:

```javascript
/**
 * Get real-time metrics for a process and its children
 * @param {number} pid - Process ID
 * @param {number} [samplePeriodMs=1000] - Sampling period for rate calculations
 * @returns {Promise<ProcessMetrics>}
 */
async function getProcessMetrics(pid, samplePeriodMs = 1000) {
  // Read /proc/{pid}/stat for CPU
  // Read /proc/{pid}/status for memory
  // Read /proc/{pid}/io for I/O rates
  // Use ss for network state
  // Aggregate across child processes
}

interface ProcessMetrics {
  pid: number;
  cpuPercent: number;
  memoryMB: number;
  ioReadBytesPerSec: number;
  ioWriteBytesPerSec: number;
  networkState: {
    established: number;
    sendQueueBytes: number;
    recvQueueBytes: number;
  };
  childCount: number;
}
```

**Leverage existing code:**
- `agent-stuck-detector.js` already has `getProcessState()` and `getNetworkState()`
- Refactor into reusable module

### Phase 2: AttachClient Status Bar (2-3 hours)

Modify `src/attach/attach-client.js`:

1. Add metrics polling interval (every 1s)
2. Render status bar using ANSI escape codes
3. Handle terminal resize to reposition bar
4. Add Ctrl+B m to toggle metrics display

```javascript
class AttachClient extends EventEmitter {
  constructor(options) {
    // ... existing code ...
    this.showMetrics = options.showMetrics ?? true;
    this.metricsInterval = null;
  }

  _startMetricsPolling() {
    this.metricsInterval = setInterval(async () => {
      const metrics = await getProcessMetrics(this.processPid);
      this._renderStatusBar(metrics);
    }, 1000);
  }

  _renderStatusBar(metrics) {
    // Save cursor position
    // Move to line 1
    // Clear line
    // Write formatted metrics
    // Restore cursor position
  }
}
```

### Phase 3: Enhanced `zeroshot watch` (3-4 hours)

Extend `src/tui/dashboard.js`:

1. Add metrics column to agent table
2. Add sparkline charts for CPU/network
3. Poll metrics for all agents in cluster
4. Add 'a' key to attach to selected agent

```javascript
// New component: AgentMetricsTable
const table = blessed.listtable({
  headers: ['AGENT', 'STATE', 'CPU', 'MEM', 'NET', 'TOKENS', 'LAST'],
  data: agents.map(a => [
    a.id,
    a.state,
    `${a.metrics.cpuPercent}%`,
    `${a.metrics.memoryMB}MB`,
    formatNetworkRate(a.metrics),
    a.tokenCount || '?',
    formatTimeSince(a.lastOutputTime)
  ])
});
```

## File Changes

| File | Change |
|------|--------|
| `src/process-metrics.js` | NEW - Metrics collection module |
| `src/attach/attach-client.js` | Add status bar rendering |
| `src/tui/dashboard.js` | Add metrics table and charts |
| `src/agent-wrapper.js` | Expose PID to callers |
| `cli/index.js` | Add `--no-metrics` flag to attach |

## Testing Strategy

1. **Unit tests** for process-metrics.js
   - Mock /proc filesystem
   - Test rate calculations
   - Test child process aggregation

2. **Integration tests**
   - Spawn real process, verify metrics
   - Test with Claude CLI running

3. **Visual testing**
   - Manual verification of TUI rendering
   - Test terminal resize handling

## Platform Support

| Platform | CPU/Mem | Network | I/O | Support Level |
|----------|---------|---------|-----|---------------|
| Linux | `/proc/stat` | `ss -tunp` | `/proc/io` | Full |
| macOS | `ps -o %cpu,rss` | `lsof -i` | N/A (requires sudo) | Full (no I/O) |
| Windows WSL | `/proc/stat` | `ss -tunp` | `/proc/io` | Full |

```javascript
// Platform-aware metrics collection
function getProcessMetrics(pid) {
  if (process.platform === 'darwin') {
    return getMetricsDarwin(pid);  // ps + lsof
  }
  return getMetricsLinux(pid);     // /proc + ss
}
```

## Open Questions

1. **Token counting** - Can we parse Claude CLI output to track token usage?
   - Yes, stream-json has `usage` events

2. **Network traffic attribution** - Per-process network bytes?
   - Not easily without eBPF
   - Use socket queue sizes as proxy for activity

3. **Historical data** - Keep history for charts?
   - Ring buffer of last 60 samples (1 minute)
   - Store in memory, not persisted

## Success Criteria

- [ ] `zeroshot attach` shows live metrics without disrupting output
- [ ] `zeroshot watch` shows all agents with metrics
- [ ] Metrics update every 1 second
- [ ] No noticeable CPU overhead (<1% additional)
- [ ] Works on Linux and degrades gracefully on macOS
