const fs = require('fs');
const os = require('os');
const path = require('path');
const lockfile = require('proper-lockfile');
const { resolveRunPlan } = require('./run-plan');

const DEFAULT_WAIT_TIMEOUT_SECONDS = 180;
const DEFAULT_WAIT_POLL_MS = 1000;
const CLUSTERS_LOCK_STALE_MS = 5000;

function getStorageDir(storageDir) {
  return storageDir || path.join(os.homedir(), '.zeroshot');
}

function getClustersFilePath(storageDir) {
  return path.join(getStorageDir(storageDir), 'clusters.json');
}

function resolveWaitTimeoutMs(waitTimeoutSeconds) {
  const parsed = Number(waitTimeoutSeconds);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return DEFAULT_WAIT_TIMEOUT_SECONDS * 1000;
  }
  return Math.floor(parsed * 1000);
}

function isProcessAlive(pid) {
  if (!pid || !Number.isInteger(pid) || pid <= 0) {
    return false;
  }
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error && error.code === 'EPERM';
  }
}

function isClusterRegistered(clusterId, storageDir) {
  const clustersFile = getClustersFilePath(storageDir);
  if (!fs.existsSync(clustersFile)) {
    return false;
  }

  let parsed;
  try {
    const raw = fs.readFileSync(clustersFile, 'utf8');
    parsed = JSON.parse(raw);
  } catch {
    return false;
  }

  return parsed !== null && typeof parsed === 'object' && Boolean(parsed[clusterId]);
}

function ensureStorageDir(storageDir) {
  const resolvedStorageDir = getStorageDir(storageDir);
  fs.mkdirSync(resolvedStorageDir, { recursive: true });
  return resolvedStorageDir;
}

function ensureClustersFile(storageDir) {
  const resolvedStorageDir = ensureStorageDir(storageDir);
  const clustersFile = getClustersFilePath(resolvedStorageDir);
  try {
    fs.writeFileSync(clustersFile, '{}', { flag: 'wx', mode: 0o600 });
  } catch (error) {
    if (!error || error.code !== 'EEXIST') {
      throw error;
    }
  }
  return clustersFile;
}

function readClustersFile(clustersFile) {
  try {
    return JSON.parse(fs.readFileSync(clustersFile, 'utf8'));
  } catch {
    return {};
  }
}

async function updateClustersFile(storageDir, updater) {
  const clustersFile = ensureClustersFile(storageDir);
  const lockfilePath = path.join(path.dirname(clustersFile), 'clusters.json.lock');
  let release;

  try {
    release = await lockfile.lock(clustersFile, {
      lockfilePath,
      stale: CLUSTERS_LOCK_STALE_MS,
      retries: {
        retries: 20,
        minTimeout: 100,
        maxTimeout: 250,
        randomize: true,
      },
    });

    const clusters = readClustersFile(clustersFile);
    const updated = updater(clusters) || clusters;
    fs.writeFileSync(clustersFile, JSON.stringify(updated, null, 2));
    return updated;
  } finally {
    if (release) {
      await release();
    }
  }
}

async function registerDetachedSetupCluster({
  clusterId,
  pid,
  storageDir,
  logPath,
  worktree,
  runOptions = {},
  cwd,
}) {
  await updateClustersFile(storageDir, (clusters) => {
    clusters[clusterId] = {
      id: clusterId,
      state: 'setup',
      createdAt: Date.now(),
      pid: Number.isInteger(pid) ? pid : null,
      setupLogPath: logPath || null,
      setupStartedAt: Date.now(),
      setupStage: 'starting',
      autoPr: resolveRunPlan(runOptions).delivery !== 'none',
      prOptions: runOptions.prBase
        ? {
            prBase: runOptions.prBase,
            mergeQueue: runOptions.mergeQueue || false,
            closeIssue: runOptions.closeIssue || null,
            cwd: cwd || null,
          }
        : null,
      worktree: worktree || null,
      config: null,
      issue: null,
      isolation: null,
      agentStates: [],
      failureInfo: null,
      provisional: true,
    };
    return clusters;
  });
}

async function markDetachedSetupFailed({ clusterId, storageDir, error, logPath }) {
  await updateClustersFile(storageDir, (clusters) => {
    const existing = clusters[clusterId] || { id: clusterId, createdAt: Date.now() };
    clusters[clusterId] = {
      ...existing,
      state: 'failed',
      pid: null,
      setupLogPath: existing.setupLogPath || logPath || null,
      setupFinishedAt: Date.now(),
      failureInfo: {
        type: 'setup',
        error: error instanceof Error ? error.message : String(error),
        timestamp: Date.now(),
      },
      provisional: true,
    };
    return clusters;
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function formatLogHint(logPath) {
  return logPath ? ` Check log: ${logPath}` : '';
}

async function waitForClusterRegistration({
  clusterId,
  timeoutMs,
  pollMs = DEFAULT_WAIT_POLL_MS,
  storageDir,
  daemonPid,
  logPath,
}) {
  const effectiveTimeoutMs =
    Number.isFinite(timeoutMs) && timeoutMs > 0 ? timeoutMs : DEFAULT_WAIT_TIMEOUT_SECONDS * 1000;
  const startedAt = Date.now();
  const deadline = startedAt + effectiveTimeoutMs;
  const logHint = formatLogHint(logPath);

  while (Date.now() < deadline) {
    if (isClusterRegistered(clusterId, storageDir)) {
      return { ready: true, elapsedMs: Date.now() - startedAt };
    }

    if (daemonPid && !isProcessAlive(daemonPid)) {
      throw new Error(
        `Detached daemon exited before cluster "${clusterId}" registered in storage.${logHint}`
      );
    }

    await sleep(pollMs);
  }

  const timeoutSeconds = Math.ceil(effectiveTimeoutMs / 1000);
  throw new Error(
    `Timed out after ${timeoutSeconds}s waiting for cluster "${clusterId}" to appear in status/list.${logHint}`
  );
}

module.exports = {
  DEFAULT_WAIT_TIMEOUT_SECONDS,
  getClustersFilePath,
  isClusterRegistered,
  isProcessAlive,
  markDetachedSetupFailed,
  registerDetachedSetupCluster,
  resolveWaitTimeoutMs,
  waitForClusterRegistration,
};
