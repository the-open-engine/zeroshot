/**
 * ProcessMetrics - Cross-platform real-time process monitoring
 *
 * Provides:
 * - CPU usage (percent)
 * - Memory usage (MB)
 * - Network I/O (bytes/sec)
 * - Process state (running, sleeping, etc.)
 * - Child process aggregation
 *
 * Supports:
 * - Linux: /proc filesystem + ss
 * - macOS: ps + lsof
 */

const { execSync } = require('./lib/safe-exec'); // Enforces timeouts
const fs = require('fs');

const PLATFORM = process.platform;

/**
 * Escape a string for safe use in shell commands
 * Prevents shell injection when passing dynamic values to execSync with shell: true
 * @param {string} str - String to escape
 * @returns {string} Shell-escaped string
 */
function escapeShell(str) {
  // Replace single quotes with escaped version and wrap in single quotes
  // This is the safest approach for shell escaping
  return `'${str.replace(/'/g, "'\\''")}'`;
}

/**
 * @typedef {Object} ProcessMetrics
 * @property {number} pid - Process ID
 * @property {boolean} exists - Whether process exists
 * @property {number} cpuPercent - CPU usage (0-100)
 * @property {number} memoryMB - Memory usage in MB
 * @property {string} state - Process state (R=running, S=sleeping, etc.)
 * @property {number} threads - Thread count
 * @property {Object} network - Network activity
 * @property {number} network.established - Established connections
 * @property {boolean} network.hasActivity - Has data in flight
 * @property {number} network.sendQueueBytes - Bytes in send queue
 * @property {number} network.recvQueueBytes - Bytes in receive queue
 * @property {number} childCount - Number of child processes
 * @property {number} timestamp - Measurement timestamp
 */

/**
 * Get all child PIDs for a process (recursive)
 * @param {number} pid - Parent process ID
 * @returns {number[]} Array of child PIDs
 */
function getChildPids(pid) {
  const children = [];

  try {
    const childPids =
      PLATFORM === 'darwin' ? collectDarwinChildPids(pid) : collectLinuxChildPids(pid);
    children.push(...childPids);

    // Recursively get grandchildren
    for (const childPid of childPids) {
      children.push(...getChildPids(childPid));
    }
  } catch {
    // Ignore errors (process may have exited)
  }

  return [...new Set(children)]; // Dedupe
}

function collectDarwinChildPids(pid) {
  const output = execSync(`pgrep -P ${escapeShell(String(pid))} 2>/dev/null`, {
    encoding: 'utf8',
    timeout: 2000,
  });

  return output.trim().split('\n').filter(Boolean).map(Number);
}

function collectLinuxChildPids(pid) {
  const taskPath = `/proc/${pid}/task`;
  if (!fs.existsSync(taskPath)) {
    return [];
  }

  const tids = fs.readdirSync(taskPath);
  const childPids = [];

  for (const tid of tids) {
    const childrenPath = `/proc/${pid}/task/${tid}/children`;
    childPids.push(...readChildPidFile(childrenPath));
  }

  return childPids;
}

function readChildPidFile(childrenPath) {
  if (!fs.existsSync(childrenPath)) {
    return [];
  }

  const raw = fs.readFileSync(childrenPath, 'utf8').trim();
  if (!raw) {
    return [];
  }

  return raw.split(/\s+/).filter(Boolean).map(Number);
}

/**
 * Get metrics for a single process (Linux)
 * @param {number} pid - Process ID
 * @returns {Object|null} Metrics or null if process doesn't exist
 */
function getProcessMetricsLinux(pid) {
  try {
    const statPath = `/proc/${pid}/stat`;
    if (!fs.existsSync(statPath)) {
      return null;
    }

    const stat = fs.readFileSync(statPath, 'utf8');
    const parts = stat.split(' ');

    // state is field 3 (index 2)
    const state = parts[2];

    // utime (14) + stime (15) = CPU ticks
    const utime = parseInt(parts[13], 10);
    const stime = parseInt(parts[14], 10);
    const cpuTicks = utime + stime;

    // Read status for memory and threads
    const status = fs.readFileSync(`/proc/${pid}/status`, 'utf8');
    const vmRss = status.match(/VmRSS:\s+(\d+)/)?.[1] || '0';
    const threads = status.match(/Threads:\s+(\d+)/)?.[1] || '1';

    return {
      pid,
      exists: true,
      state,
      cpuTicks,
      memoryKB: parseInt(vmRss, 10),
      threads: parseInt(threads, 10),
    };
  } catch {
    return null;
  }
}

/**
 * Get metrics for a single process (macOS)
 * @param {number} pid - Process ID
 * @returns {Object|null} Metrics or null if process doesn't exist
 */
function getProcessMetricsDarwin(pid) {
  try {
    // ps -p PID -o %cpu=,rss=,state=
    const output = execSync(`ps -p ${escapeShell(String(pid))} -o %cpu=,rss=,state= 2>/dev/null`, {
      encoding: 'utf8',
      timeout: 2000,
    });

    if (!output.trim()) {
      return null;
    }

    const parts = output.trim().split(/\s+/);
    const cpuPercent = parseFloat(parts[0]) || 0;
    const rssKB = parseInt(parts[1], 10) || 0;
    const state = parts[2]?.[0] || 'S'; // First char (R, S, etc.)

    return {
      pid,
      exists: true,
      state,
      cpuPercent, // macOS ps gives us percent directly
      memoryKB: rssKB,
      threads: 1, // ps doesn't give thread count easily
    };
  } catch {
    return null;
  }
}

/**
 * Get network state for a process (Linux)
 * Uses ss -tip to get extended TCP info including cumulative bytes sent/received
 * @param {number} pid - Process ID
 * @returns {Object} Network state
 */
function getNetworkStateLinux(pid) {
  const result = {
    established: 0,
    hasActivity: false,
    sendQueueBytes: 0,
    recvQueueBytes: 0,
    bytesSent: 0, // Cumulative bytes sent across all sockets
    bytesReceived: 0, // Cumulative bytes received across all sockets
  };

  try {
    // Use ss -tip to get extended TCP info with bytes_sent/bytes_received
    // -t = TCP only, -i = show internal TCP info, -p = show process
    const output = execSync(
      `ss -tip 2>/dev/null | grep -A1 "pid=${escapeShell(String(pid))}," || true`,
      {
        encoding: 'utf8',
        timeout: 3000,
      }
    );

    if (!output.trim()) {
      return result;
    }

    const lines = output.trim().split('\n');

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];

      // Parse socket line: State  Recv-Q  Send-Q  Local:Port  Peer:Port  Process
      const match = line.match(/^(\S+)\s+(\d+)\s+(\d+)\s+/);
      if (match) {
        const state = match[1];
        const recvQ = parseInt(match[2], 10);
        const sendQ = parseInt(match[3], 10);

        if (state === 'ESTAB') {
          result.established++;
        }

        result.recvQueueBytes += recvQ;
        result.sendQueueBytes += sendQ;

        if (recvQ > 0 || sendQ > 0) {
          result.hasActivity = true;
        }
      }

      // Parse extended TCP info line (follows socket line)
      // Contains: bytes_sent:N bytes_received:N (and other metrics)
      const bytesSentMatch = line.match(/bytes_sent:(\d+)/);
      const bytesReceivedMatch = line.match(/bytes_received:(\d+)/);

      if (bytesSentMatch) {
        result.bytesSent += parseInt(bytesSentMatch[1], 10);
        result.hasActivity = true;
      }
      if (bytesReceivedMatch) {
        result.bytesReceived += parseInt(bytesReceivedMatch[1], 10);
        result.hasActivity = true;
      }
    }
  } catch {
    // Ignore errors
  }

  return result;
}

/**
 * Get network state for a process (macOS)
 * Note: macOS doesn't expose per-socket byte counts like Linux's ss -tip
 * We return 0 for bytesSent/bytesReceived (not available without dtrace/nettop)
 * @param {number} pid - Process ID
 * @returns {Object} Network state
 */
function getNetworkStateDarwin(pid) {
  const result = {
    established: 0,
    hasActivity: false,
    sendQueueBytes: 0,
    recvQueueBytes: 0,
    bytesSent: 0, // Not available on macOS without root/dtrace
    bytesReceived: 0, // Not available on macOS without root/dtrace
  };

  try {
    // lsof -i -n -P for network connections
    const output = execSync(`lsof -i -n -P -a -p ${escapeShell(String(pid))} 2>/dev/null || true`, {
      encoding: 'utf8',
      timeout: 3000,
    });

    if (!output.trim()) {
      return result;
    }

    const lines = output.trim().split('\n').slice(1); // Skip header

    for (const line of lines) {
      const parts = line.split(/\s+/);
      // Look for ESTABLISHED connections
      if (parts.includes('ESTABLISHED') || parts.some((p) => p.includes('->'))) {
        result.established++;
        result.hasActivity = true; // lsof doesn't show queue sizes, assume activity
      }
    }
  } catch {
    // Ignore errors
  }

  return result;
}

/**
 * Get real-time metrics for a process and its children
 * @param {number} pid - Process ID
 * @param {Object} [options] - Options
 * @param {number} [options.samplePeriodMs=1000] - Sample period for rate calculations (Linux only)
 * @returns {Promise<ProcessMetrics>}
 */
function getProcessMetrics(pid, options = {}) {
  const samplePeriodMs = options.samplePeriodMs || 1000;

  if (PLATFORM === 'darwin') {
    return getProcessMetricsDarwinAggregated(pid);
  }

  return getProcessMetricsLinuxAggregated(pid, samplePeriodMs);
}

/**
 * Get aggregated metrics for process tree (Linux)
 * @param {number} pid - Root process ID
 * @param {number} samplePeriodMs - Sample period for CPU calculation
 * @returns {Promise<ProcessMetrics>}
 */
async function getProcessMetricsLinuxAggregated(pid, samplePeriodMs) {
  // Get initial CPU sample
  const allPids = [pid, ...getChildPids(pid)];
  const t0Metrics = {};

  for (const p of allPids) {
    const m = getProcessMetricsLinux(p);
    if (m) t0Metrics[p] = m;
  }

  if (Object.keys(t0Metrics).length === 0) {
    return {
      pid,
      exists: false,
      cpuPercent: 0,
      memoryMB: 0,
      state: 'X',
      threads: 0,
      network: {
        established: 0,
        hasActivity: false,
        sendQueueBytes: 0,
        recvQueueBytes: 0,
        bytesSent: 0,
        bytesReceived: 0,
      },
      childCount: 0,
      timestamp: Date.now(),
    };
  }

  // Wait for sample period
  await new Promise((r) => setTimeout(r, samplePeriodMs));

  // Get second CPU sample
  const t1Metrics = {};
  const currentPids = [pid, ...getChildPids(pid)];

  for (const p of currentPids) {
    const m = getProcessMetricsLinux(p);
    if (m) t1Metrics[p] = m;
  }

  // Calculate aggregated metrics
  let totalCpuTicksDelta = 0;
  let totalMemoryKB = 0;
  let totalThreads = 0;
  let rootState = 'S';

  for (const p of Object.keys(t1Metrics)) {
    const t1 = t1Metrics[p];
    const t0 = t0Metrics[p];

    if (t0 && t1) {
      totalCpuTicksDelta += t1.cpuTicks - t0.cpuTicks;
    }

    totalMemoryKB += t1.memoryKB;
    totalThreads += t1.threads;

    if (p === String(pid)) {
      rootState = t1.state;
    }
  }

  // Calculate CPU percent
  const clockTicks = 100; // Usually 100 on Linux
  const cpuSeconds = totalCpuTicksDelta / clockTicks;
  const sampleSeconds = samplePeriodMs / 1000;
  const rawCpuPercent = (cpuSeconds / sampleSeconds) * 100;

  // Normalize to per-core average (0-100% range)
  const cpuCores = require('os').cpus().length;
  const cpuPercent = Math.min(100, rawCpuPercent / cpuCores);

  // Get network state for all processes
  let network = {
    established: 0,
    hasActivity: false,
    sendQueueBytes: 0,
    recvQueueBytes: 0,
    bytesSent: 0,
    bytesReceived: 0,
  };
  for (const p of Object.keys(t1Metrics)) {
    const netState = getNetworkStateLinux(parseInt(p, 10));
    network.established += netState.established;
    network.sendQueueBytes += netState.sendQueueBytes;
    network.recvQueueBytes += netState.recvQueueBytes;
    network.bytesSent += netState.bytesSent;
    network.bytesReceived += netState.bytesReceived;
    if (netState.hasActivity) network.hasActivity = true;
  }

  return {
    pid,
    exists: true,
    cpuPercent: parseFloat(cpuPercent.toFixed(1)),
    memoryMB: parseFloat((totalMemoryKB / 1024).toFixed(1)),
    state: rootState,
    threads: totalThreads,
    network,
    childCount: Object.keys(t1Metrics).length - 1,
    timestamp: Date.now(),
  };
}

/**
 * Get aggregated metrics for process tree (macOS)
 * @param {number} pid - Root process ID
 * @returns {Promise<ProcessMetrics>}
 */
function getProcessMetricsDarwinAggregated(pid) {
  const allPids = [pid, ...getChildPids(pid)];
  let totalCpuPercent = 0;
  let totalMemoryKB = 0;
  let totalThreads = 0;
  let rootState = 'S';
  let existsCount = 0;

  for (const p of allPids) {
    const m = getProcessMetricsDarwin(p);
    if (m) {
      existsCount++;
      totalCpuPercent += m.cpuPercent;
      totalMemoryKB += m.memoryKB;
      totalThreads += m.threads;

      if (p === pid) {
        rootState = m.state;
      }
    }
  }

  if (existsCount === 0) {
    return {
      pid,
      exists: false,
      cpuPercent: 0,
      memoryMB: 0,
      state: 'X',
      threads: 0,
      network: { established: 0, hasActivity: false, sendQueueBytes: 0, recvQueueBytes: 0 },
      childCount: 0,
      timestamp: Date.now(),
    };
  }

  // Get network state
  let network = {
    established: 0,
    hasActivity: false,
    sendQueueBytes: 0,
    recvQueueBytes: 0,
    bytesSent: 0,
    bytesReceived: 0,
  };
  for (const p of allPids) {
    const netState = getNetworkStateDarwin(p);
    network.established += netState.established;
    network.bytesSent += netState.bytesSent;
    network.bytesReceived += netState.bytesReceived;
    if (netState.hasActivity) network.hasActivity = true;
  }

  // Normalize to per-core average (0-100% range)
  const cpuCores = require('os').cpus().length;
  const normalizedCpu = Math.min(100, totalCpuPercent / cpuCores);

  return {
    pid,
    exists: true,
    cpuPercent: parseFloat(normalizedCpu.toFixed(1)),
    memoryMB: parseFloat((totalMemoryKB / 1024).toFixed(1)),
    state: rootState,
    threads: totalThreads,
    network,
    childCount: existsCount - 1,
    timestamp: Date.now(),
  };
}

/**
 * Format metrics for display
 * @param {ProcessMetrics} metrics
 * @returns {string} Formatted string
 */
function formatMetrics(metrics) {
  if (!metrics.exists) {
    return '(process exited)';
  }

  const parts = [];

  // CPU
  parts.push(`CPU: ${metrics.cpuPercent}%`);

  // Memory
  parts.push(`Mem: ${metrics.memoryMB}MB`);

  // Network
  if (metrics.network.established > 0) {
    parts.push(`Net: ${metrics.network.established} conn`);
    if (metrics.network.hasActivity) {
      parts.push('↕');
    }
  }

  // Children
  if (metrics.childCount > 0) {
    parts.push(`+${metrics.childCount} child`);
  }

  return parts.join(' │ ');
}

/**
 * Get state icon for process state
 * @param {string} state - Process state char
 * @returns {string} Icon
 */
function getStateIcon(state) {
  switch (state) {
    case 'R':
      return '🟢'; // Running
    case 'S':
      return '🔵'; // Sleeping
    case 'D':
      return '🟡'; // Disk wait
    case 'Z':
      return '💀'; // Zombie
    case 'T':
      return '⏸️'; // Stopped
    case 'X':
      return '❌'; // Dead
    default:
      return '⚪';
  }
}

/**
 * Check if platform is supported for full metrics
 * @returns {boolean}
 */
function isPlatformSupported() {
  return PLATFORM === 'linux' || PLATFORM === 'darwin';
}

/**
 * Get platform-specific metrics provider info
 * @returns {Object}
 */
function getPlatformInfo() {
  return {
    platform: PLATFORM,
    supported: isPlatformSupported(),
    cpuSource: PLATFORM === 'linux' ? '/proc/stat' : 'ps',
    memorySource: PLATFORM === 'linux' ? '/proc/status' : 'ps',
    networkSource: PLATFORM === 'linux' ? 'ss' : 'lsof',
    ioSupported: PLATFORM === 'linux', // I/O only on Linux
  };
}

module.exports = {
  getProcessMetrics,
  getChildPids,
  formatMetrics,
  getStateIcon,
  isPlatformSupported,
  getPlatformInfo,
  // Export internal functions for testing
  getProcessMetricsLinux,
  getProcessMetricsDarwin,
  getNetworkStateLinux,
  getNetworkStateDarwin,
};
