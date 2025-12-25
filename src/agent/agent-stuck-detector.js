/**
 * AgentStuckDetector - Multi-indicator process health analysis
 *
 * Detects stuck Claude processes using multiple indicators:
 * - Process state (S=sleeping vs R=running)
 * - Wait channel (ep_poll = blocked on epoll_wait)
 * - CPU usage over sample period
 * - Context switches (activity indicator)
 * - Network socket state (data in flight)
 *
 * CRITICAL: Single-indicator detection (just output freshness) has HIGH false positive risk.
 * Multi-indicator approach ONLY flags processes that fail ALL indicators.
 *
 * Scoring system:
 * - isSleeping: +1
 * - isBlockedOnPoll: +1
 * - lowCpuUsage: +1
 * - lowCtxSwitches: +1
 * - noDataInFlight: +0.5 (secondary signal)
 * - hasSynSent: +1 (stuck trying to connect)
 * - hasDataInFlight: -2 (active I/O = working)
 *
 * Threshold: stuckScore >= 3.5 = likely stuck
 */

const { execSync } = require('child_process');
const fs = require('fs');

// Stuck detection thresholds
const STUCK_THRESHOLD = 3.5; // Score at which we consider process stuck
const HIGH_CONFIDENCE_THRESHOLD = 4.5;
const CPU_LOW_THRESHOLD = 1; // Percent - below this is considered "low"
const CTX_SWITCHES_LOW_THRESHOLD = 10; // Below this is considered "inactive"

/**
 * Get process state from /proc filesystem
 * @param {number} pid - Process ID
 * @returns {object} Process state info
 */
function getProcessState(pid) {
  try {
    const statPath = `/proc/${pid}/stat`;
    if (!fs.existsSync(statPath)) {
      return { exists: false };
    }

    const stat = fs.readFileSync(statPath, 'utf8');
    const parts = stat.split(' ');

    // stat fields: pid, comm, state, ppid, pgrp, ...
    // State is the 3rd field (index 2): R=running, S=sleeping, D=disk sleep, Z=zombie
    const state = parts[2];

    // Get wchan (what the process is waiting on)
    let wchan = '';
    try {
      wchan = fs.readFileSync(`/proc/${pid}/wchan`, 'utf8').trim();
    } catch {
      // wchan may not be readable
    }

    // Get CPU usage from stat
    // utime (field 14) + stime (field 15) = total CPU ticks
    const utime = parseInt(parts[13], 10);
    const stime = parseInt(parts[14], 10);

    // Get status for more info
    const status = fs.readFileSync(`/proc/${pid}/status`, 'utf8');
    const threads = status.match(/Threads:\s+(\d+)/)?.[1] || '1';
    const volCtxSwitches = status.match(/voluntary_ctxt_switches:\s+(\d+)/)?.[1] || '0';

    return {
      exists: true,
      state,
      wchan,
      cpuTicks: utime + stime,
      threads: parseInt(threads, 10),
      volCtxSwitches: parseInt(volCtxSwitches, 10),
    };
  } catch (err) {
    return { exists: false, error: err.message };
  }
}

/**
 * Get network socket activity for a process
 * @param {number} pid - Process ID
 * @returns {object} Network state info
 */
function getNetworkState(pid) {
  try {
    const fdPath = `/proc/${pid}/fd`;
    if (!fs.existsSync(fdPath)) {
      return { hasNetwork: false };
    }

    // Use ss to get socket states for this process
    let ssOutput = '';
    try {
      ssOutput = execSync(`ss -tunp 2>/dev/null | grep ",pid=${pid}," || true`, {
        encoding: 'utf8',
        timeout: 5000,
      });
    } catch {
      return { hasNetwork: false };
    }

    if (!ssOutput.trim()) {
      return { hasNetwork: false, connections: [] };
    }

    const connections = [];
    const lines = ssOutput.trim().split('\n');

    for (const line of lines) {
      // Parse ss output: State  Recv-Q  Send-Q  Local Address:Port  Peer Address:Port  Process
      const match = line.match(/^(\S+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(\S+)/);
      if (match) {
        connections.push({
          state: match[1],
          recvQ: parseInt(match[2], 10),
          sendQ: parseInt(match[3], 10),
          local: match[4],
          peer: match[5],
        });
      }
    }

    // Analyze connection health
    const establishedCount = connections.filter((c) => c.state === 'ESTAB').length;
    const hasDataInFlight = connections.some((c) => c.recvQ > 0 || c.sendQ > 0);
    const hasSynSent = connections.some((c) => c.state === 'SYN-SENT');

    return {
      hasNetwork: connections.length > 0,
      connections,
      establishedCount,
      hasDataInFlight,
      hasSynSent,
    };
  } catch (err) {
    return { hasNetwork: false, error: err.message };
  }
}

/**
 * Analyze process health using multi-indicator approach
 *
 * @param {number} pid - Process ID
 * @param {number} samplePeriodMs - How long to sample (default 5000ms)
 * @returns {Promise<object>} Analysis result with isLikelyStuck, stuckScore, indicators
 */
async function analyzeProcessHealth(pid, samplePeriodMs = 5000) {
  const t0 = getProcessState(pid);
  if (!t0.exists) {
    return { isLikelyStuck: null, reason: 'Process does not exist', pid };
  }

  // Wait and sample again
  await new Promise((r) => setTimeout(r, samplePeriodMs));

  const t1 = getProcessState(pid);
  if (!t1.exists) {
    return { isLikelyStuck: null, reason: 'Process died during analysis', pid };
  }

  // Calculate CPU usage during sample period
  const cpuTicksDelta = t1.cpuTicks - t0.cpuTicks;
  const ctxSwitchesDelta = t1.volCtxSwitches - t0.volCtxSwitches;

  // Get clock ticks per second (typically 100 on Linux)
  const clockTicks = 100;

  // CPU seconds used during sample
  const cpuSeconds = cpuTicksDelta / clockTicks;
  const sampleSeconds = samplePeriodMs / 1000;
  const cpuPercent = (cpuSeconds / sampleSeconds) * 100;

  // Get network state
  const network = getNetworkState(pid);

  // Analyze stuck indicators
  const indicators = {
    isSleeping: t1.state === 'S',
    isBlockedOnPoll: t1.wchan.includes('poll') || t1.wchan.includes('wait'),
    lowCpuUsage: cpuPercent < CPU_LOW_THRESHOLD,
    lowCtxSwitches: ctxSwitchesDelta < CTX_SWITCHES_LOW_THRESHOLD,
    // Network indicators (only apply if process has network connections)
    noDataInFlight: network.hasNetwork && !network.hasDataInFlight,
    hasSynSent: network.hasSynSent, // Stuck trying to connect
  };

  // Calculate stuck score using weighted indicators
  let stuckScore = 0;
  if (indicators.isSleeping) stuckScore += 1;
  if (indicators.isBlockedOnPoll) stuckScore += 1;
  if (indicators.lowCpuUsage) stuckScore += 1;
  if (indicators.lowCtxSwitches) stuckScore += 1;
  if (indicators.noDataInFlight) stuckScore += 0.5; // Secondary signal
  if (indicators.hasSynSent) stuckScore += 1; // Strong signal - stuck connecting

  // CRITICAL: If data IS flowing, REDUCE stuck score (legitimate work)
  if (network.hasDataInFlight) {
    stuckScore = Math.max(0, stuckScore - 2); // Active I/O = likely working
  }

  const isLikelyStuck = stuckScore >= STUCK_THRESHOLD;
  const confidence =
    stuckScore >= HIGH_CONFIDENCE_THRESHOLD
      ? 'high'
      : stuckScore >= STUCK_THRESHOLD
        ? 'medium'
        : 'low';

  return {
    pid,
    state: t1.state,
    wchan: t1.wchan,
    cpuPercent: parseFloat(cpuPercent.toFixed(2)),
    ctxSwitchesDelta,
    threads: t1.threads,
    network: {
      hasConnections: network.hasNetwork,
      establishedCount: network.establishedCount || 0,
      hasDataInFlight: network.hasDataInFlight || false,
      hasSynSent: network.hasSynSent || false,
    },
    indicators,
    stuckScore: parseFloat(stuckScore.toFixed(1)),
    isLikelyStuck,
    confidence,
    analysis: isLikelyStuck
      ? `Process appears STUCK: sleeping on ${t1.wchan}, ${cpuPercent.toFixed(1)}% CPU, ${ctxSwitchesDelta} ctx switches`
      : `Process appears WORKING: ${cpuPercent.toFixed(1)}% CPU, ${ctxSwitchesDelta} ctx switches, state=${t1.state}`,
  };
}

/**
 * Check if we're on a platform that supports /proc filesystem
 * @returns {boolean}
 */
function isPlatformSupported() {
  return process.platform === 'linux' && fs.existsSync('/proc');
}

module.exports = {
  analyzeProcessHealth,
  getProcessState,
  getNetworkState,
  isPlatformSupported,
  // Export thresholds for testing
  STUCK_THRESHOLD,
  HIGH_CONFIDENCE_THRESHOLD,
  CPU_LOW_THRESHOLD,
  CTX_SWITCHES_LOW_THRESHOLD,
};
