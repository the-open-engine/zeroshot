const fs = require('fs');
const os = require('os');
const path = require('path');

const DEFAULT_WAIT_TIMEOUT_SECONDS = 180;
const DEFAULT_WAIT_POLL_MS = 1000;

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
    Number.isFinite(timeoutMs) && timeoutMs > 0
      ? timeoutMs
      : DEFAULT_WAIT_TIMEOUT_SECONDS * 1000;
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
  resolveWaitTimeoutMs,
  waitForClusterRegistration,
};
