const fs = require('fs');
const os = require('os');
const path = require('path');
const Database = require('better-sqlite3');

const ACTIVE_STATES = new Set(['initializing', 'running']);
const TERMINAL_MESSAGE_TOPICS = {
  completed: 'CLUSTER_COMPLETE',
  failed: 'CLUSTER_FAILED',
  agentError: 'AGENT_ERROR',
};
const MAX_SUMMARY_LENGTH = 96;
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

function safeJsonParse(text) {
  if (typeof text !== 'string' || text.length === 0) {
    return null;
  }
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function clip(text, maxLength = MAX_SUMMARY_LENGTH) {
  const compact = String(text || '').replace(/\s+/g, ' ').trim();
  if (compact.length <= maxLength) {
    return compact;
  }
  return `${compact.slice(0, maxLength - 3)}...`;
}

function extractIssueTitle(text) {
  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim());
  const titleIndex = lines.findIndex((line) => /^##\s+Title\b/i.test(line));
  if (titleIndex >= 0) {
    const title = lines.slice(titleIndex + 1).find((line) => line.length > 0);
    if (title) {
      return title;
    }
  }
  return null;
}

function summarizeTaskText(text, contentData) {
  const issueTitle =
    contentData && typeof contentData.title === 'string' ? contentData.title.trim() : '';
  if (issueTitle.length > 0) {
    return clip(issueTitle);
  }

  const extractedIssueTitle = extractIssueTitle(text);
  if (extractedIssueTitle) {
    return clip(extractedIssueTitle);
  }

  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .filter((line) => !/^#+\s/.test(line));
  if (lines.length > 0) {
    return clip(lines[0]);
  }

  return null;
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
  if (activeTask && typeof activeTask.id === 'string') {
    return activeTask.id;
  }

  const busyAgent = agentStates.find((agent) => {
    if (!agent || typeof agent.state !== 'string') {
      return false;
    }
    return !['idle', 'completed', 'stopped'].includes(agent.state);
  });
  if (busyAgent && typeof busyAgent.id === 'string') {
    return busyAgent.id;
  }

  return null;
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
  const length = stat.size - start;
  const fd = fs.openSync(filePath, 'r');
  try {
    const buffer = Buffer.alloc(length);
    fs.readSync(fd, buffer, 0, length, start);
    return buffer.toString('utf8');
  } finally {
    fs.closeSync(fd);
  }
}

function deriveSetupFailure(text) {
  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0) {
    return { state: 'initializing', failureReason: null };
  }

  const interestingLine =
    [...lines]
      .reverse()
      .find(
        (line) =>
          /^Error:\s+Command failed:/i.test(line) ||
          /^npm error\b/i.test(line) ||
          /^fatal:/i.test(line) ||
          /No such file or directory/i.test(line) ||
          /not found$/i.test(line)
      ) || null;

  if (interestingLine) {
    return {
      state: 'setup_failed',
      failureReason: clip(interestingLine, 180),
    };
  }

  return { state: 'initializing', failureReason: null };
}

function readDbHistory(clusterId, dbPath) {
  if (!fs.existsSync(dbPath)) {
    return null;
  }

  const db = new Database(dbPath, { readonly: true, timeout: 5000 });
  try {
    const countRow = db
      .prepare('SELECT COUNT(*) AS count FROM messages WHERE cluster_id = ?')
      .get(clusterId);
    const messageCount = Number(countRow?.count || 0);
    if (messageCount === 0) {
      return { messageCount: 0 };
    }

    const issueOpened = db
      .prepare(
        `SELECT timestamp, content_text, content_data
         FROM messages
         WHERE cluster_id = ? AND topic = 'ISSUE_OPENED'
         ORDER BY timestamp ASC
         LIMIT 1`
      )
      .get(clusterId);

    const firstMessage = db
      .prepare(
        `SELECT timestamp
         FROM messages
         WHERE cluster_id = ?
         ORDER BY timestamp ASC
         LIMIT 1`
      )
      .get(clusterId);

    const lastMessage = db
      .prepare(
        `SELECT timestamp, topic, sender, content_text
         FROM messages
         WHERE cluster_id = ?
         ORDER BY timestamp DESC
         LIMIT 1`
      )
      .get(clusterId);

    const hasComplete = Boolean(
      db
        .prepare(
          `SELECT 1
           FROM messages
           WHERE cluster_id = ? AND topic = ?
           LIMIT 1`
        )
        .get(clusterId, TERMINAL_MESSAGE_TOPICS.completed)
    );
    const hasClusterFailed = Boolean(
      db
        .prepare(
          `SELECT 1
           FROM messages
           WHERE cluster_id = ? AND topic = ?
           LIMIT 1`
        )
        .get(clusterId, TERMINAL_MESSAGE_TOPICS.failed)
    );
    const hasAgentError = Boolean(
      db
        .prepare(
          `SELECT 1
           FROM messages
           WHERE cluster_id = ? AND topic = ?
           LIMIT 1`
        )
        .get(clusterId, TERMINAL_MESSAGE_TOPICS.agentError)
    );

    const issueData = safeJsonParse(issueOpened?.content_data || '');
    return {
      messageCount,
      createdAt: firstMessage?.timestamp ?? issueOpened?.timestamp ?? null,
      lastActivityAt: lastMessage?.timestamp ?? null,
      lastTopic: lastMessage?.topic ?? null,
      lastSender: lastMessage?.sender ?? null,
      issue: parsePositiveInteger(issueData?.issue_number),
      taskSummary: summarizeTaskText(issueOpened?.content_text || '', issueData),
      hasComplete,
      hasClusterFailed,
      hasAgentError,
    };
  } finally {
    db.close();
  }
}

function classifyRegistryState(registryEntry, isProcessRunning) {
  const rawState = registryEntry?.state || null;
  const daemonPid = parsePositiveInteger(registryEntry?.daemonPid ?? registryEntry?.pid);
  let state = rawState;
  let orphaned = false;

  if (state === 'running' || state === 'initializing') {
    if (daemonPid === undefined || !isProcessRunning(daemonPid)) {
      state = 'zombie';
    }
  } else if (daemonPid !== undefined && isProcessRunning(daemonPid)) {
    orphaned = true;
  }

  return {
    state,
    rawState,
    daemonPid,
    orphaned,
  };
}

function buildRunSummary({
  clusterId,
  storageDir = defaultStorageDir(),
  registryEntry = null,
  isProcessRunning = defaultIsProcessRunning,
}) {
  const dbPath = path.join(storageDir, `${clusterId}.db`);
  const daemonLogPath = path.join(storageDir, `${clusterId}-daemon.log`);
  const dbHistory = readDbHistory(clusterId, dbPath);
  const registry = classifyRegistryState(registryEntry, isProcessRunning);
  const daemonLogExists = fs.existsSync(daemonLogPath);

  if ((dbHistory === null || dbHistory.messageCount === 0) && daemonLogExists) {
    const logText = readTail(daemonLogPath);
    const setup = deriveSetupFailure(logText);
    const logStat = fs.statSync(daemonLogPath);
    const lastActivityAt = logStat.mtimeMs;
    const isStaleSetup =
      setup.state === 'initializing' && Date.now() - lastActivityAt >= STALE_SETUP_MS;
    return {
      id: clusterId,
      state: isStaleSetup ? 'setup_failed' : setup.state,
      rawState: registry.rawState,
      issue: registryEntry?.issue ?? undefined,
      createdAt: registryEntry?.createdAt ?? logStat.birthtimeMs ?? logStat.mtimeMs,
      lastActivityAt,
      currentAgent: pickCurrentAgent(registryEntry?.agentStates),
      taskSummary: null,
      messageCount: 0,
      orphaned: registry.orphaned,
      daemonPid: registry.daemonPid ?? null,
      failureReason:
        setup.failureReason ||
        (isStaleSetup ? 'Setup never reached ISSUE_OPENED and appears stalled.' : null),
      source: 'setup-log',
    };
  }

  if (dbHistory === null) {
    return null;
  }

  let state = registry.state;
  if (dbHistory.hasComplete) {
    state = 'completed';
  } else if (dbHistory.hasClusterFailed || dbHistory.hasAgentError) {
    state = 'failed';
  } else if (!state) {
    state = 'stopped';
  }

  const stat = fs.existsSync(dbPath) ? fs.statSync(dbPath) : null;
  return {
    id: clusterId,
    state,
    rawState: registry.rawState,
    issue: registryEntry?.issue ?? dbHistory.issue,
    createdAt: registryEntry?.createdAt ?? dbHistory.createdAt ?? stat?.birthtimeMs ?? stat?.mtimeMs,
    lastActivityAt: dbHistory.lastActivityAt ?? stat?.mtimeMs ?? null,
    currentAgent: pickCurrentAgent(registryEntry?.agentStates),
    taskSummary: dbHistory.taskSummary,
    messageCount: dbHistory.messageCount,
    orphaned: registry.orphaned,
    daemonPid: registry.daemonPid ?? null,
    failureReason: null,
    source: registryEntry ? 'registry' : 'history',
  };
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

function listRunSummaries({
  storageDir = defaultStorageDir(),
  since = null,
  today = false,
  activeOnly = false,
  limit = null,
  isProcessRunning = defaultIsProcessRunning,
} = {}) {
  const registry = readRegistry(storageDir);
  const clusterIds = new Set([
    ...Object.keys(registry),
    ...listIdsBySuffix(storageDir, '.db'),
    ...listIdsBySuffix(storageDir, '-daemon.log'),
  ]);

  const sinceMs = today ? startOfToday() : coerceSince(since);
  const runs = [];
  for (const clusterId of clusterIds) {
    const summary = buildRunSummary({
      clusterId,
      storageDir,
      registryEntry: registry[clusterId] || null,
      isProcessRunning,
    });
    if (!summary) {
      continue;
    }
    if (sinceMs !== null && isFiniteNumber(summary.createdAt) && summary.createdAt < sinceMs) {
      continue;
    }
    if (activeOnly && !ACTIVE_STATES.has(summary.state)) {
      continue;
    }
    runs.push(summary);
  }

  runs.sort((left, right) => {
    const leftTime = left.createdAt || 0;
    const rightTime = right.createdAt || 0;
    return rightTime - leftTime;
  });

  if (Number.isInteger(limit) && limit > 0) {
    return runs.slice(0, limit);
  }
  return runs;
}

module.exports = {
  ACTIVE_STATES,
  buildRunSummary,
  defaultIsProcessRunning,
  defaultStorageDir,
  listRunSummaries,
  readRegistry,
  summarizeTaskText,
};
