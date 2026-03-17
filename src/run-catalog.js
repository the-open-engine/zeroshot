const fs = require('fs');
const os = require('os');
const path = require('path');

const { deriveSetupFailure, readDbHistory } = require('./run-catalog-history');

const ACTIVE_STATES = new Set(['initializing', 'running']);
const MAX_LOG_TAIL_BYTES = 64 * 1024;
const STALE_SETUP_MS = 5 * 60 * 1000;

function defaultStorageDir() {
  const homeDir =
    process.env.ZEROSHOT_HOME || process.env.HOME || process.env.USERPROFILE || os.homedir();
  return path.join(homeDir, '.zeroshot');
}

function isFiniteNumber(value) {
  return typeof value === 'number' && Number.isFinite(value);
}

function parsePositiveInteger(value) {
  if (typeof value === 'number' && Number.isInteger(value) && value > 0) {
    return value;
  }
  if (typeof value === 'string') {
    const parsed = Number.parseInt(value, 10);
    if (Number.isInteger(parsed) && parsed > 0) {
      return parsed;
    }
  }
  return undefined;
}

function pickCurrentAgent(agentStates) {
  if (!Array.isArray(agentStates)) {
    return null;
  }

  const activeTask = agentStates.find((agent) => agent && agent.currentTask === true);
  if (activeTask?.id) {
    return activeTask.id;
  }

  const busyAgent = agentStates.find(
    (agent) =>
      agent &&
      typeof agent.state === 'string' &&
      !['idle', 'completed', 'stopped'].includes(agent.state)
  );
  return busyAgent?.id || null;
}

function defaultIsProcessRunning(pid) {
  if (!Number.isInteger(pid) || pid <= 0) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error && error.code === 'EPERM';
  }
}

function readRegistry(storageDir = defaultStorageDir()) {
  const clustersFile = path.join(storageDir, 'clusters.json');
  if (!fs.existsSync(clustersFile)) {
    return {};
  }
  try {
    const parsed = JSON.parse(fs.readFileSync(clustersFile, 'utf8'));
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch {
    return {};
  }
}

function listIdsBySuffix(storageDir, suffix) {
  if (!fs.existsSync(storageDir)) {
    return [];
  }
  return fs
    .readdirSync(storageDir, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(suffix))
    .map((entry) => entry.name.slice(0, -suffix.length))
    .filter((id) => id.length > 0);
}

function readTail(filePath, maxBytes = MAX_LOG_TAIL_BYTES) {
  if (!fs.existsSync(filePath)) {
    return '';
  }
  const stat = fs.statSync(filePath);
  if (stat.size === 0) {
    return '';
  }
  const start = Math.max(0, stat.size - maxBytes);
  const fd = fs.openSync(filePath, 'r');
  try {
    const buffer = Buffer.alloc(stat.size - start);
    fs.readSync(fd, buffer, 0, buffer.length, start);
    return buffer.toString('utf8');
  } finally {
    fs.closeSync(fd);
  }
}

function buildRunningState(rawState, daemonPid, isProcessRunning) {
  if (rawState !== 'running' && rawState !== 'initializing') {
    return rawState;
  }
  return daemonPid === undefined || !isProcessRunning(daemonPid) ? 'zombie' : rawState;
}

function classifyRegistryState(registryEntry, isProcessRunning) {
  const rawState = registryEntry?.state || null;
  const daemonPid = parsePositiveInteger(registryEntry?.daemonPid ?? registryEntry?.pid);
  return {
    state: buildRunningState(rawState, daemonPid, isProcessRunning),
    rawState,
    daemonPid,
    orphaned:
      rawState !== 'running' &&
      rawState !== 'initializing' &&
      daemonPid !== undefined &&
      isProcessRunning(daemonPid),
  };
}

function buildSetupState(setupState, lastActivityAt) {
  if (setupState !== 'initializing') {
    return setupState;
  }
  return Date.now() - lastActivityAt >= STALE_SETUP_MS ? 'setup_failed' : 'initializing';
}

function buildSetupFailureReason(setupFailureReason, state) {
  if (setupFailureReason) {
    return setupFailureReason;
  }
  if (state === 'setup_failed') {
    return 'Setup never reached ISSUE_OPENED and appears stalled.';
  }
  return null;
}

function buildSummaryTimestamp(primaryValue, fallbackValue) {
  if (primaryValue !== undefined && primaryValue !== null) {
    return primaryValue;
  }
  return fallbackValue ?? null;
}

function buildSetupLogSummary({ clusterId, registry, registryEntry, daemonLogPath }) {
  const logText = readTail(daemonLogPath);
  const setup = deriveSetupFailure(logText);
  const logStat = fs.statSync(daemonLogPath);
  const lastActivityAt = logStat.mtimeMs;
  const state = buildSetupState(setup.state, lastActivityAt);
  return {
    id: clusterId,
    state,
    rawState: registry.rawState,
    issue: registryEntry?.issue ?? undefined,
    createdAt: buildSummaryTimestamp(
      registryEntry?.createdAt,
      logStat.birthtimeMs ?? logStat.mtimeMs
    ),
    lastActivityAt,
    currentAgent: pickCurrentAgent(registryEntry?.agentStates),
    taskSummary: null,
    messageCount: 0,
    orphaned: registry.orphaned,
    daemonPid: registry.daemonPid ?? null,
    failureReason: buildSetupFailureReason(setup.failureReason, state),
    source: 'setup-log',
  };
}

function attachWarning(summary, warning) {
  if (!summary || !warning) {
    return summary;
  }
  return { ...summary, warning };
}

function deriveFallbackState({ registry, hasDaemonLog }) {
  if (registry.state) {
    return registry.state;
  }
  return hasDaemonLog ? 'setup_failed' : 'unknown';
}

function buildFallbackFailureReason(daemonLogPath, state) {
  if (state !== 'setup_failed' || !fs.existsSync(daemonLogPath)) {
    return null;
  }
  return deriveSetupFailure(readTail(daemonLogPath)).failureReason;
}

function buildSqliteFallbackSummary({
  clusterId,
  dbPath,
  daemonLogPath,
  registry,
  registryEntry,
  dbHistory,
}) {
  const dbStat = fs.existsSync(dbPath) ? fs.statSync(dbPath) : null;
  const daemonLogStat = fs.existsSync(daemonLogPath) ? fs.statSync(daemonLogPath) : null;
  const state = deriveFallbackState({ registry, hasDaemonLog: Boolean(daemonLogStat) });

  return {
    id: clusterId,
    state,
    rawState: registry.rawState,
    issue: registryEntry?.issue ?? undefined,
    createdAt: buildSummaryTimestamp(
      registryEntry?.createdAt,
      dbStat?.birthtimeMs ?? dbStat?.mtimeMs ?? daemonLogStat?.birthtimeMs ?? daemonLogStat?.mtimeMs
    ),
    lastActivityAt: buildSummaryTimestamp(dbStat?.mtimeMs, daemonLogStat?.mtimeMs),
    currentAgent: pickCurrentAgent(registryEntry?.agentStates),
    taskSummary: null,
    messageCount: null,
    orphaned: registry.orphaned,
    daemonPid: registry.daemonPid ?? null,
    failureReason: buildFallbackFailureReason(daemonLogPath, state),
    source: registryEntry ? 'registry-fallback' : 'history-fallback',
    warning: dbHistory.sqliteWarning,
  };
}

function deriveSummaryState({ registry, dbHistory, inMemory }) {
  if (inMemory || ACTIVE_STATES.has(registry.state)) {
    return registry.state;
  }
  if (dbHistory.hasComplete) {
    return 'completed';
  }
  if (dbHistory.hasClusterFailed || dbHistory.hasAgentError) {
    return 'failed';
  }
  return registry.state || 'stopped';
}

function buildDbCreatedAt(registryEntry, dbHistory, stat) {
  return buildSummaryTimestamp(
    registryEntry?.createdAt,
    dbHistory.createdAt ?? stat?.birthtimeMs ?? stat?.mtimeMs
  );
}

function buildDbLastActivityAt(dbHistory, stat) {
  return buildSummaryTimestamp(dbHistory.lastActivityAt, stat?.mtimeMs);
}

function buildDbSummary({ clusterId, dbPath, dbHistory, registry, registryEntry, inMemory }) {
  const stat = fs.existsSync(dbPath) ? fs.statSync(dbPath) : null;
  return {
    id: clusterId,
    state: deriveSummaryState({ registry, dbHistory, inMemory }),
    rawState: registry.rawState,
    issue: registryEntry?.issue ?? dbHistory.issue,
    createdAt: buildDbCreatedAt(registryEntry, dbHistory, stat),
    lastActivityAt: buildDbLastActivityAt(dbHistory, stat),
    currentAgent: pickCurrentAgent(registryEntry?.agentStates),
    taskSummary: dbHistory.taskSummary,
    messageCount: dbHistory.messageCount,
    orphaned: registry.orphaned,
    daemonPid: registry.daemonPid ?? null,
    failureReason: dbHistory.dbFailureReason ?? null,
    source: registryEntry ? 'registry' : 'history',
  };
}

function buildRunSummary({
  clusterId,
  storageDir = defaultStorageDir(),
  registryEntry = null,
  isProcessRunning = defaultIsProcessRunning,
  inMemory = false,
  readDbHistoryFn = readDbHistory,
}) {
  const dbPath = path.join(storageDir, `${clusterId}.db`);
  const daemonLogPath = path.join(storageDir, `${clusterId}-daemon.log`);
  const dbHistory = readDbHistoryFn(clusterId, dbPath);
  const registry = classifyRegistryState(registryEntry, isProcessRunning);

  if (dbHistory?.sqliteUnavailable) {
    return buildSqliteFallbackSummary({
      clusterId,
      dbPath,
      daemonLogPath,
      registry,
      registryEntry,
      dbHistory,
    });
  }

  if ((dbHistory === null || dbHistory.messageCount === 0) && fs.existsSync(daemonLogPath)) {
    return buildSetupLogSummary({ clusterId, registry, registryEntry, daemonLogPath });
  }

  if (dbHistory === null) {
    return null;
  }

  return attachWarning(
    buildDbSummary({ clusterId, dbPath, dbHistory, registry, registryEntry, inMemory }),
    dbHistory.sqliteWarning
  );
}

function coerceSince(value) {
  if (value === undefined || value === null || value === '') {
    return null;
  }
  if (isFiniteNumber(value)) {
    return value;
  }
  const parsed = Date.parse(String(value));
  return Number.isFinite(parsed) ? parsed : null;
}

function startOfToday() {
  const date = new Date();
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

function shouldIncludeSummary(summary, sinceMs, activeOnly) {
  if (!summary) {
    return false;
  }
  if (sinceMs !== null && isFiniteNumber(summary.createdAt) && summary.createdAt < sinceMs) {
    return false;
  }
  if (activeOnly && !ACTIVE_STATES.has(summary.state)) {
    return false;
  }
  return true;
}

function collectClusterIds(storageDir, registry) {
  return new Set([
    ...Object.keys(registry),
    ...listIdsBySuffix(storageDir, '.db'),
    ...listIdsBySuffix(storageDir, '-daemon.log'),
  ]);
}

function buildSummaryList({ clusterIds, storageDir, registry, isProcessRunning }) {
  return [...clusterIds].map((clusterId) =>
    buildRunSummary({
      clusterId,
      storageDir,
      registryEntry: registry[clusterId] || null,
      isProcessRunning,
    })
  );
}

function sortRunSummaries(runs) {
  return runs.sort((left, right) => (right.createdAt || 0) - (left.createdAt || 0));
}

function applySummaryLimit(runs, limit) {
  if (!Number.isInteger(limit) || limit <= 0) {
    return runs;
  }
  return runs.slice(0, limit);
}

function listRunSummaries({
  storageDir = defaultStorageDir(),
  since = null,
  today = false,
  activeOnly = false,
  limit = null,
  isProcessRunning = defaultIsProcessRunning,
} = {}) {
  const registry = readRegistry(storageDir);
  const clusterIds = collectClusterIds(storageDir, registry);
  const sinceMs = today ? startOfToday() : coerceSince(since);
  const runs = buildSummaryList({ clusterIds, storageDir, registry, isProcessRunning }).filter(
    (summary) => shouldIncludeSummary(summary, sinceMs, activeOnly)
  );
  sortRunSummaries(runs);

  return applySummaryLimit(runs, limit);
}

module.exports = {
  ACTIVE_STATES,
  buildRunSummary,
  defaultIsProcessRunning,
  defaultStorageDir,
  listRunSummaries,
  readRegistry,
};
